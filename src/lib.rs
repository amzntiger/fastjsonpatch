//! Rust core of fastjsonpatch: the two hot paths of `jsonpatch`.
//!
//! - `diff` is a port of `jsonpatch.DiffBuilder` that yields the same operations in
//!   the same order. Scalars are compared natively instead of through `json.dumps`,
//!   and paths live on a stack instead of being rebuilt as tuples per node. Inputs it
//!   cannot reproduce exactly (non-JSON types, dict/list subclasses, deep nesting)
//!   make it return `None`, and the Python layer runs the reference implementation.
//! - `apply` runs the operations on exact dicts/lists. An operation that would fail
//!   or touches anything else is handed to jsonpatch's own operation class before
//!   anything is mutated, so errors and edge cases behave like jsonpatch.

use ahash::AHashMap;
use pyo3::ffi;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

/// Deeper documents go to the reference implementation (which raises RecursionError).
const MAX_DEPTH: usize = 1000;

enum Fail {
    /// The input needs the reference implementation.
    Unsupported,
    Py(PyErr),
}

impl From<PyErr> for Fail {
    fn from(e: PyErr) -> Self {
        Fail::Py(e)
    }
}

impl From<pyo3::CastError<'_, '_>> for Fail {
    fn from(e: pyo3::CastError<'_, '_>) -> Self {
        Fail::Py(e.into())
    }
}

type R<T> = Result<T, Fail>;

/// Lets signal handlers (Ctrl-C) run during long loops: `tick` is cheap and asks
/// Python for pending signals once every 1024 calls.
struct Ticker<'py> {
    py: Python<'py>,
    n: u32,
}

impl<'py> Ticker<'py> {
    fn new(py: Python<'py>) -> Self {
        Ticker { py, n: 0 }
    }

    fn tick(&mut self) -> PyResult<()> {
        self.n = self.n.wrapping_add(1);
        if self.n % 1024 == 0 {
            self.py.check_signals()
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- value kinds

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Dict,
    List,
    Str,
    Int,
    Float,
    Bool,
    Null,
    Other,
}

fn kind(o: &Bound<'_, PyAny>) -> Kind {
    if o.is_exact_instance_of::<PyDict>() {
        Kind::Dict
    } else if o.is_exact_instance_of::<PyList>() {
        Kind::List
    } else if o.is_exact_instance_of::<PyString>() {
        Kind::Str
    } else if o.is_exact_instance_of::<PyInt>() {
        Kind::Int
    } else if o.is_exact_instance_of::<PyFloat>() {
        Kind::Float
    } else if o.is_exact_instance_of::<PyBool>() {
        Kind::Bool
    } else if o.is_none() {
        Kind::Null
    } else {
        Kind::Other
    }
}

/// Would `json.dumps(o)` succeed? (keys may be str/int/float/bool/None)
fn serializable(o: &Bound<'_, PyAny>, depth: usize) -> PyResult<bool> {
    if depth > MAX_DEPTH {
        return Ok(false);
    }
    Ok(match kind(o) {
        Kind::Dict => {
            for (k, v) in o.cast::<PyDict>()?.iter() {
                if matches!(kind(&k), Kind::Dict | Kind::List | Kind::Other) || !serializable(&v, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        Kind::List => {
            for v in o.cast::<PyList>()?.iter() {
                if !serializable(&v, depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        Kind::Other => false,
        _ => true,
    })
}

/// A float with a whole value, which `_number` makes an int.
fn whole(x: f64) -> bool {
    x.is_finite() && x.fract() == 0.0
}

/// `a` and `b`, both ints or floats, are the same number (RFC 6902, 4.6): 1 and 1.0
/// are, every NaN is the same as every NaN (as in their `json.dumps`).
fn same_number(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, ka: Kind, kb: Kind) -> R<bool> {
    if ka == Kind::Int {
        int_text(a)?;
    }
    if kb == Kind::Int {
        int_text(b)?;
    }
    if ka == Kind::Float && kb == Kind::Float {
        let (x, y) = (a.cast::<PyFloat>()?.value(), b.cast::<PyFloat>()?.value());
        return Ok((x.is_nan() && y.is_nan()) || x == y);
    }
    Ok(a.is(b) || a.eq(b)?)
}

fn is_number(k: Kind) -> bool {
    matches!(k, Kind::Int | Kind::Float)
}

/// `_same_scalar` for values that are not both dicts or both lists.
fn json_eq(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, depth: usize) -> R<bool> {
    let (ka, kb) = (kind(a), kind(b));
    if ka == Kind::Other || kb == Kind::Other {
        return Err(Fail::Unsupported);
    }
    let container = |k| matches!(k, Kind::Dict | Kind::List);
    if container(ka) || container(kb) {
        // different kinds always serialize differently, as long as both serialize
        for (o, k) in [(a, ka), (b, kb)] {
            if container(k) && !serializable(o, depth)? {
                return Err(Fail::Unsupported);
            }
        }
        return Ok(false);
    }
    if is_number(ka) && is_number(kb) {
        return same_number(a, b, ka, kb);
    }
    if ka != kb {
        return Ok(false); // e.g. 1 vs true vs "1"
    }
    Ok(match ka {
        Kind::Null => true,
        _ => a.eq(b)?,
    })
}

/// Next `(key, value)` of a dict, borrowed; no allocation or refcounting.
fn dict_next(d: &Bound<'_, PyDict>, pos: &mut ffi::Py_ssize_t) -> Option<(*mut ffi::PyObject, *mut ffi::PyObject)> {
    let (mut k, mut v) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: `d` is a live dict and the GIL is held; the pointers are borrowed from it.
    (unsafe { ffi::PyDict_Next(d.as_ptr(), pos, &mut k, &mut v) } != 0).then_some((k, v))
}

fn same_keys_in_order(a: &Bound<'_, PyDict>, b: &Bound<'_, PyDict>) -> PyResult<bool> {
    let (mut pa, mut pb) = (0, 0);
    while let (Some((x, _)), Some((y, _))) = (dict_next(a, &mut pa), dict_next(b, &mut pb)) {
        if x == y {
            continue;
        }
        // SAFETY: borrowed from the dicts, which outlive this loop.
        let (x, y) = unsafe { (Borrowed::from_ptr(a.py(), x), Borrowed::from_ptr(a.py(), y)) };
        // only exact str keys: their equality cannot run Python code or disagree with hashing
        let (Ok(x), Ok(y)) = (x.cast_exact::<PyString>(), y.cast_exact::<PyString>()) else { return Ok(false) };
        if !x.as_any().eq(y.as_any())? {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------- paths

/// An object key or an array index, as `DiffBuilder` keeps them in "parts".
#[derive(Clone)]
enum Part<'py> {
    /// always valid UTF-8 (checked by `key_part`)
    Key(Bound<'py, PyString>),
    Idx(i64),
}

fn key_str<'a>(k: &'a Bound<'_, PyString>) -> &'a str {
    k.to_str().unwrap_or_default()
}

impl PartialEq for Part<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Part::Idx(a), Part::Idx(b)) => a == b,
            (Part::Key(a), Part::Key(b)) => a.is(b) || key_str(a) == key_str(b),
            _ => false,
        }
    }
}

type Path<'py> = Vec<Part<'py>>;

fn is_prefix(sub: &[Part], parts: &[Part]) -> bool {
    sub.len() <= parts.len() && sub == &parts[..sub.len()]
}

fn is_inside(parts: &[Part], container: &[Part]) -> bool {
    parts.len() > container.len() && is_prefix(container, parts)
}

/// `parts` assumes an item at `location`; adjust it to the item missing.
fn without_item<'py>(parts: &[Part<'py>], location: &[Part<'py>]) -> Path<'py> {
    let depth = location.len() - 1;
    if let (Part::Idx(loc), true) = (&location[depth], is_inside(parts, &location[..depth])) {
        if let Part::Idx(i) = parts[depth] {
            if i > *loc {
                let mut p = parts.to_vec();
                p[depth] = Part::Idx(i - 1);
                return p;
            }
        }
    }
    parts.to_vec()
}

fn escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '~' => out.push_str("~0"),
            '/' => out.push_str("~1"),
            _ => out.push(c),
        }
    }
}

fn to_pointer(path: &[Part]) -> String {
    let mut s = String::new();
    for p in path {
        s.push('/');
        match p {
            Part::Idx(i) => s.push_str(&i.to_string()),
            Part::Key(k) => escape_into(&mut s, key_str(k)),
        }
    }
    s
}

/// A part of a location: like a `Part`, but an array item instead of its index, so
/// it stays the same item when items are added or removed in front of it.
#[derive(Clone)]
enum LPart<'py> {
    Key(Bound<'py, PyString>),
    Item(usize),
}

impl PartialEq for LPart<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (LPart::Item(a), LPart::Item(b)) => a == b,
            (LPart::Key(a), LPart::Key(b)) => a.is(b) || key_str(a) == key_str(b),
            _ => false,
        }
    }
}

impl Eq for LPart<'_> {}

impl std::hash::Hash for LPart<'_> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        match self {
            LPart::Key(k) => key_str(k).hash(h),
            LPart::Item(i) => i.hash(h),
        }
    }
}

type Loc<'py> = Vec<LPart<'py>>;

// ---------------------------------------------------------------- array items

/// See `_ArrayItem`.
struct Item {
    array: usize,
    label: i64,
    initial: bool,
}

/// See `_ArrayItems`; holds item ids.
#[derive(Default)]
struct Array {
    items: Vec<usize>,
    labels: Vec<i64>,
    present: Vec<usize>,
    present_labels: Vec<i64>,
}

const LABEL_STEP: i64 = 1 << 16;
const LABEL_GAP: i64 = 1 << 32;

// ---------------------------------------------------------------- move keys

/// Appends an encoding of `o` that is equal for two values exactly when
/// `json.dumps(_sorted_members(value))` is. Values it cannot encode like that
/// (non-str keys, non-JSON types, huge ints, deep nesting) are `Unsupported`.
fn encode(o: &Bound<'_, PyAny>, depth: usize, out: &mut Vec<u8>) -> R<()> {
    if depth > MAX_DEPTH {
        return Err(Fail::Unsupported);
    }
    let put_str = |out: &mut Vec<u8>, s: &str| {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    match kind(o) {
        Kind::Dict => {
            let d = o.cast::<PyDict>()?;
            let mut members = Vec::with_capacity(d.len());
            for (k, v) in d.iter() {
                let k = k.cast_into_exact::<PyString>().map_err(|_| Fail::Unsupported)?;
                k.to_str().map_err(|_| Fail::Unsupported)?;
                members.push((k, v));
            }
            members.sort_by(|a, b| key_str(&a.0).cmp(key_str(&b.0)));
            out.push(b'{');
            out.extend_from_slice(&(members.len() as u64).to_le_bytes());
            for (k, v) in &members {
                put_str(out, key_str(k));
                encode(v, depth + 1, out)?;
            }
        }
        Kind::List => {
            let l = o.cast::<PyList>()?;
            out.push(b'[');
            out.extend_from_slice(&(l.len() as u64).to_le_bytes());
            for v in l.iter() {
                encode(&v, depth + 1, out)?;
            }
        }
        Kind::Str => {
            out.push(b's');
            put_str(out, o.cast::<PyString>()?.to_str().map_err(|_| Fail::Unsupported)?);
        }
        Kind::Int => {
            out.push(b'i');
            match o.extract::<i64>() {
                Ok(i) => put_str(out, &i.to_string()),
                // str() of a huge int raises, like json.dumps does
                Err(_) => put_str(out, o.str().map_err(|_| Fail::Unsupported)?.to_str()?),
            }
        }
        Kind::Float => {
            let x = o.cast::<PyFloat>()?.value();
            if x.is_nan() {
                out.push(b'n'); // every NaN dumps as "NaN"
            } else if whole(x) {
                // the same as the int of that value
                out.push(b'i');
                if x.abs() < 9.0e18 {
                    put_str(out, &(x as i64).to_string());
                } else {
                    put_str(out, o.call_method0("__int__")?.str()?.to_str()?);
                }
            } else {
                out.push(b'f');
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
        }
        Kind::Bool => out.push(if o.is_truthy()? { b't' } else { b'F' }),
        Kind::Null => out.push(b'z'),
        Kind::Other => return Err(Fail::Unsupported),
    }
    Ok(())
}

fn move_key(o: &Bound<'_, PyAny>) -> R<Vec<u8>> {
    let mut out = vec![];
    encode(o, 0, &mut out)?;
    Ok(out)
}

/// The JSON text of an int: `None` for a huge one, whose `str()` (and json.dumps) raises.
fn int_text(o: &Bound<'_, PyAny>) -> R<Option<i64>> {
    match o.extract::<i64>() {
        Ok(i) => Ok(Some(i)),
        Err(_) => {
            o.str().map_err(|_| Fail::Unsupported)?;
            Ok(None)
        }
    }
}

fn borrow<'a, 'py>(py: Python<'py>, p: *mut ffi::PyObject) -> Borrowed<'a, 'py, PyAny> {
    // SAFETY: callers pass items borrowed from a live dict or list, with the GIL held.
    unsafe { Borrowed::from_ptr(py, p) }
}

fn list_item<'a, 'py>(l: &'a Bound<'py, PyList>, i: usize) -> Borrowed<'a, 'py, PyAny> {
    // SAFETY: `i` is in bounds; PyList_GetItem returns a borrowed reference.
    borrow(l.py(), unsafe { ffi::PyList_GetItem(l.as_ptr(), i as ffi::Py_ssize_t) })
}

fn exact_str_key(k: &Bound<'_, PyAny>) -> R<()> {
    if k.is_exact_instance_of::<PyString>() { Ok(()) } else { Err(Fail::Unsupported) }
}

/// A hash of `o` that is equal for values whose move keys are equal; `Unsupported`
/// for non-JSON values, non-str keys, huge ints and deep nesting.
fn json_hash(o: &Bound<'_, PyAny>, depth: usize, rs: &ahash::RandomState) -> R<u64> {
    use std::hash::{BuildHasher, Hash, Hasher};
    if depth > MAX_DEPTH {
        return Err(Fail::Unsupported);
    }
    let mut h = rs.build_hasher();
    match kind(o) {
        Kind::Dict => {
            // members in any order hash the same
            let d = o.cast::<PyDict>()?;
            let (mut pos, mut sum) = (0, 0u64);
            while let Some((k, v)) = dict_next(d, &mut pos) {
                let k = borrow(o.py(), k);
                exact_str_key(&k)?;
                let mut m = rs.build_hasher();
                (k.hash()?, json_hash(&borrow(o.py(), v), depth + 1, rs)?).hash(&mut m);
                sum = sum.wrapping_add(m.finish());
            }
            (b'{', sum).hash(&mut h);
        }
        Kind::List => {
            let l = o.cast::<PyList>()?;
            b'['.hash(&mut h);
            for i in 0..l.len() {
                json_hash(&list_item(l, i), depth + 1, rs)?.hash(&mut h);
            }
        }
        Kind::Str => (b's', o.hash()?).hash(&mut h),
        Kind::Int => {
            int_text(o)?;
            (b'i', o.hash()?).hash(&mut h);
        }
        Kind::Float => {
            let x = o.cast::<PyFloat>()?.value();
            if whole(x) {
                // hash(1.0) == hash(1): the same as the int of that value
                (b'i', o.hash()?).hash(&mut h);
            } else {
                (b'f', if x.is_nan() { 0 } else { x.to_bits() }, x.is_nan()).hash(&mut h);
            }
        }
        Kind::Bool => (b'b', o.is_truthy()?).hash(&mut h),
        Kind::Null => b'z'.hash(&mut h),
        Kind::Other => return Err(Fail::Unsupported),
    }
    Ok(h.finish())
}

/// Do `a` and `b` have the same move key? A comparison that ends early does not
/// check the rest of the values for being supported.
fn same_json(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, depth: usize) -> R<bool> {
    if depth > MAX_DEPTH {
        return Err(Fail::Unsupported);
    }
    let (ka, kb) = (kind(a), kind(b));
    if ka == Kind::Other || kb == Kind::Other {
        return Err(Fail::Unsupported);
    }
    if is_number(ka) && is_number(kb) {
        return same_number(a, b, ka, kb);
    }
    if ka != kb {
        return Ok(false);
    }
    let py = a.py();
    Ok(match ka {
        Kind::Dict => {
            let (x, y) = (a.cast::<PyDict>()?, b.cast::<PyDict>()?);
            if x.len() != y.len() {
                return Ok(false);
            }
            // Usually both have their keys in the same order
            let (mut px, mut pyy) = (0, 0);
            while let (Some((k1, v1)), Some((k2, v2))) = (dict_next(x, &mut px), dict_next(y, &mut pyy)) {
                let k1 = borrow(py, k1);
                exact_str_key(&k1)?;
                if !(k1.as_ptr() == k2 || (borrow(py, k2).is_exact_instance_of::<PyString>() && k1.eq(borrow(py, k2))?)) {
                    return same_members(x, y, depth);
                }
                if !same_json(&borrow(py, v1), &borrow(py, v2), depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        Kind::List => {
            let (x, y) = (a.cast::<PyList>()?, b.cast::<PyList>()?);
            if x.len() != y.len() {
                return Ok(false);
            }
            for i in 0..x.len() {
                if !same_json(&list_item(x, i), &list_item(y, i), depth + 1)? {
                    return Ok(false);
                }
            }
            true
        }
        Kind::Str => a.is(b) || a.eq(b)?,
        Kind::Bool => a.is(b),
        Kind::Null => true,
        Kind::Int | Kind::Float | Kind::Other => unreachable!(),
    })
}

/// `same_json` for dicts of the same size whose keys are in different orders.
fn same_members(x: &Bound<'_, PyDict>, y: &Bound<'_, PyDict>, depth: usize) -> R<bool> {
    let py = x.py();
    let mut pos = 0;
    while let Some((k, v)) = dict_next(x, &mut pos) {
        let k = borrow(py, k);
        exact_str_key(&k)?;
        match y.get_item(&*k)? {
            Some(w) if same_json(&borrow(py, v), &w, depth + 1)? => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// `_positional_cost`
fn positional_cost(old: &Bound<'_, PyAny>, new: &Bound<'_, PyAny>) -> PyResult<u64> {
    if old.eq(new)? {
        return Ok(0);
    }
    match (kind(old), kind(new)) {
        (Kind::Dict, Kind::Dict) => {
            let (o, n) = (old.cast::<PyDict>()?, new.cast::<PyDict>()?);
            let mut c = 0;
            for (k, v) in o.iter() {
                c += match n.get_item(&k)? {
                    Some(w) => positional_cost(&v, &w)?,
                    None => 1,
                };
            }
            for k in n.keys() {
                c += !o.contains(&k)? as u64;
            }
            Ok(c)
        }
        (Kind::List, Kind::List) => {
            let (o, n) = (old.cast::<PyList>()?, new.cast::<PyList>()?);
            let mut c = o.len().abs_diff(n.len()) as u64;
            for (v, w) in o.iter().zip(n.iter()) {
                c += positional_cost(&v, &w)?;
            }
            Ok(c)
        }
        _ => Ok(1),
    }
}

/// Ids of `items`, equal for items with equal move keys, continuing from `n_ids`.
fn intern<'a, 'py>(
    items: &'a [Bound<'py, PyAny>],
    buckets: &mut AHashMap<u64, Vec<(u32, &'a Bound<'py, PyAny>)>>,
    n_ids: &mut u32,
    rs: &ahash::RandomState,
    ticker: &mut Ticker,
) -> R<Vec<u32>> {
    let mut out = Vec::with_capacity(items.len());
    for o in items {
        ticker.tick()?;
        let bucket = buckets.entry(json_hash(o, 0, rs)?).or_default();
        let mut id = None;
        for &(i, c) in bucket.iter() {
            if same_json(c, o, 0)? {
                id = Some(i);
                break;
            }
        }
        out.push(match id {
            Some(i) => i,
            None => {
                bucket.push((*n_ids, o));
                *n_ids += 1;
                *n_ids - 1
            }
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- SequenceMatcher

/// `difflib.SequenceMatcher(None, a, b, autojunk).get_opcodes()`, without the
/// 'equal' ones, as `(i1, i2, j1, j2)`. Items are ids below `n_ids`.
fn differing_opcodes(a: &[u32], b: &[u32], n_ids: usize, autojunk: bool, ticker: &mut Ticker) -> PyResult<Vec<(usize, usize, usize, usize)>> {
    let mut b2j: Vec<Vec<usize>> = vec![vec![]; n_ids];
    for (j, &x) in b.iter().enumerate() {
        b2j[x as usize].push(j);
    }
    if autojunk && b.len() >= 200 {
        let ntest = b.len() / 100 + 1;
        for idxs in &mut b2j {
            if idxs.len() > ntest {
                idxs.clear(); // popular
            }
        }
    }

    let mut find_longest_match = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> PyResult<(usize, usize, usize)> {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: AHashMap<usize, usize> = AHashMap::new();
        let mut newj2len: AHashMap<usize, usize> = AHashMap::new();
        for i in alo..ahi {
            ticker.tick()?;
            newj2len.clear();
            for &j in &b2j[a[i] as usize] {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = if j == 0 { 0 } else { j2len.get(&(j - 1)).copied().unwrap_or(0) } + 1;
                newj2len.insert(j, k);
                if k > bestsize {
                    (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                }
            }
            std::mem::swap(&mut j2len, &mut newj2len);
        }
        // there is no junk, so only this extension applies
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        Ok((besti, bestj, bestsize))
    };

    let (la, lb) = (a.len(), b.len());
    let mut queue = vec![(0, la, 0, lb)];
    let mut blocks = vec![];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = find_longest_match(alo, ahi, blo, bhi)?;
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort_unstable();
    blocks.push((la, lb, 0));

    // adjacent blocks only matter for the 'equal' opcodes, which are dropped
    let (mut i, mut j) = (0, 0);
    let mut runs = vec![];
    for (ai, bj, size) in blocks {
        if i < ai || j < bj {
            runs.push((i, ai, j, bj));
        }
        (i, j) = (ai + size, bj + size);
    }
    Ok(runs)
}

// ---------------------------------------------------------------- diff

#[derive(Clone, Copy, PartialEq)]
enum OpKind {
    Add,
    Remove,
    Replace,
    Move,
}

struct Op<'py> {
    kind: OpKind,
    path: Loc<'py>,
    from: Option<Loc<'py>>,
    value: Option<Bound<'py, PyAny>>,
    prev: usize,
    next: usize,
}

const ST_ADD: usize = 0;
const ST_REMOVE: usize = 1;

/// Up to which product of their lengths two lists have all equal items matched up.
const EXACT_MATCH_LIMIT: u128 = 500 * 500;

struct Builder<'py> {
    /// doubly linked list of operations; node 0 is the sentinel
    ops: Vec<Op<'py>>,
    /// move key -> nodes of the operations that add (or remove) such a value
    storage: [AHashMap<Vec<u8>, Vec<usize>>; 2],
    /// the array at each location
    arrays: AHashMap<Loc<'py>, usize>,
    array_data: Vec<Array>,
    items: Vec<Item>,
    path: Path<'py>,
    ticker: Ticker<'py>,
}

impl<'py> Builder<'py> {
    fn new(py: Python<'py>) -> Self {
        let root = Op { kind: OpKind::Add, path: vec![], from: None, value: None, prev: 0, next: 0 };
        Builder {
            ops: vec![root],
            storage: [AHashMap::new(), AHashMap::new()],
            arrays: AHashMap::new(),
            array_data: vec![],
            items: vec![],
            path: vec![],
            ticker: Ticker::new(py),
        }
    }

    // ------------------------------------------------ _ArrayItems

    fn new_item(&mut self, array: usize, label: i64, initial: bool) -> usize {
        self.items.push(Item { array, label, initial });
        self.items.len() - 1
    }

    /// `_ArrayItems.at`
    fn at(&mut self, a: usize, index: usize) -> usize {
        while self.array_data[a].present.len() <= index {
            let label = self.array_data[a].labels.last().map_or(0, |l| l + LABEL_GAP);
            let item = self.new_item(a, label, true);
            let arr = &mut self.array_data[a];
            arr.items.push(item);
            arr.labels.push(label);
            arr.present.push(item);
            arr.present_labels.push(label);
        }
        self.array_data[a].present[index]
    }

    /// `_ArrayItems.insert`
    fn insert_item(&mut self, a: usize, index: usize) -> usize {
        let mut position = 0;
        if index > 0 {
            let before = self.at(a, index - 1);
            let before = self.items[before].label;
            position = self.array_data[a].labels.partition_point(|&l| l <= before);
        }
        let n = self.array_data[a].labels.len();
        let label = if position == 0 || position == n {
            let labels = &self.array_data[a].labels;
            if labels.is_empty() {
                0
            } else if position == 0 {
                labels[0] - LABEL_GAP
            } else {
                labels[n - 1] + LABEL_GAP
            }
        } else {
            if self.array_data[a].labels[position] - self.array_data[a].labels[position - 1] < 2 {
                self.relabel(a);
            }
            let labels = &self.array_data[a].labels;
            let (low, high) = (labels[position - 1], labels[position]);
            low + LABEL_STEP.min((high - low) / 2)
        };
        let item = self.new_item(a, label, false);
        let arr = &mut self.array_data[a];
        arr.items.insert(position, item);
        arr.labels.insert(position, label);
        self.add_present(item);
        item
    }

    fn relabel(&mut self, a: usize) {
        let arr = &mut self.array_data[a];
        arr.labels = (0..arr.items.len() as i64).map(|i| i * LABEL_GAP).collect();
        for (&item, &label) in arr.items.iter().zip(&arr.labels) {
            self.items[item].label = label;
        }
        arr.present_labels = arr.present.iter().map(|&i| self.items[i].label).collect();
    }

    fn add_present(&mut self, item: usize) {
        let label = self.items[item].label;
        let arr = &mut self.array_data[self.items[item].array];
        let position = arr.present_labels.partition_point(|&l| l < label);
        arr.present.insert(position, item);
        arr.present_labels.insert(position, label);
    }

    fn remove_present(&mut self, item: usize) -> R<()> {
        let label = self.items[item].label;
        let arr = &mut self.array_data[self.items[item].array];
        let position = arr.present_labels.partition_point(|&l| l < label);
        if position >= arr.present.len() {
            return Err(Fail::Unsupported); // jsonpatch raises IndexError
        }
        arr.present.remove(position);
        arr.present_labels.remove(position);
        Ok(())
    }

    /// `_ArrayItem.index`
    fn index_of(&self, item: usize) -> i64 {
        let label = self.items[item].label;
        self.array_data[self.items[item].array].present_labels.partition_point(|&l| l < label) as i64
    }

    // ------------------------------------------------ locations

    fn array_at(&mut self, loc: &[LPart<'py>]) -> usize {
        if let Some(&a) = self.arrays.get(loc) {
            return a;
        }
        self.array_data.push(Array::default());
        let a = self.array_data.len() - 1;
        self.arrays.insert(loc.to_vec(), a);
        a
    }

    /// `_location`
    fn location(&mut self, parts: &[Part<'py>]) -> Loc<'py> {
        let mut loc = Vec::with_capacity(parts.len());
        for part in parts {
            loc.push(match part {
                Part::Key(k) => LPart::Key(k.clone()),
                Part::Idx(i) => {
                    let a = self.array_at(&loc);
                    LPart::Item(self.at(a, *i as usize))
                }
            });
        }
        loc
    }

    /// `_new_location`
    fn new_location(&mut self, key: Part<'py>) -> Loc<'py> {
        let path = self.path.clone();
        let mut loc = self.location(&path);
        loc.push(match key {
            Part::Key(k) => LPart::Key(k),
            Part::Idx(i) => {
                let a = self.array_at(&loc);
                LPart::Item(self.insert_item(a, i as usize))
            }
        });
        loc
    }

    /// `_parts`
    fn parts(&self, loc: &[LPart<'py>]) -> Path<'py> {
        loc.iter()
            .map(|p| match p {
                LPart::Key(k) => Part::Key(k.clone()),
                LPart::Item(i) => Part::Idx(self.index_of(*i)),
            })
            .collect()
    }

    /// `_set_present`
    fn set_present(&mut self, loc: &[LPart<'py>], present: bool) -> R<()> {
        if let Some(&LPart::Item(item)) = loc.last() {
            if present {
                self.add_present(item);
            } else {
                self.remove_present(item)?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------ operations

    fn take_index(&mut self, key: &[u8], st: usize) -> Option<usize> {
        self.storage[st].get_mut(key).and_then(Vec::pop)
    }

    fn store_index(&mut self, key: Vec<u8>, node: usize, st: usize) {
        self.storage[st].entry(key).or_default().push(node);
    }

    fn insert(&mut self, kind: OpKind, path: Loc<'py>, from: Option<Loc<'py>>, value: Option<Bound<'py, PyAny>>) -> usize {
        let id = self.ops.len();
        let last = self.ops[0].prev;
        self.ops.push(Op { kind, path, from, value, prev: last, next: 0 });
        self.ops[last].next = id;
        self.ops[0].prev = id;
        id
    }

    fn remove(&mut self, id: usize) {
        let (prev, next) = (self.ops[id].prev, self.ops[id].next);
        self.ops[prev].next = next;
        self.ops[next].prev = prev;
    }

    fn joined(&self, key: Part<'py>) -> Path<'py> {
        let mut p = self.path.clone();
        p.push(key);
        p
    }

    fn item_added(&mut self, key: Part<'py>, item: Bound<'py, PyAny>) -> R<()> {
        self.ticker.tick()?;
        let target = self.joined(key.clone());
        let mkey = move_key(&item)?;
        if let Some(index) = self.take_index(&mkey, ST_REMOVE) {
            let removed = self.ops[index].path.clone();
            // where the removed item is if it is not removed
            let source = self.parts(&removed);
            // RFC 6902 does not allow moving a value into its own children
            if !is_inside(&target, &source) {
                self.remove(index);
                if source != target {
                    let to = self.new_location(key);
                    self.insert(OpKind::Move, to, Some(removed), None);
                } else {
                    self.set_present(&removed, true)?;
                }
                return Ok(());
            }
        }
        let to = self.new_location(key);
        let node = self.insert(OpKind::Add, to, None, Some(item));
        self.store_index(mkey, node, ST_ADD);
        Ok(())
    }

    fn item_removed(&mut self, key: Part<'py>, item: Bound<'py, PyAny>) -> R<()> {
        self.ticker.tick()?;
        let source = self.joined(key);
        let location = self.location(&source);
        let mkey = move_key(&item)?;
        if let Some(index) = self.take_index(&mkey, ST_ADD) {
            let added_location = self.ops[index].path.clone();
            let added = self.parts(&added_location);
            self.remove(index);
            if without_item(&source, &added) != added {
                self.set_present(&location, false)?;
                self.insert(OpKind::Move, added_location, Some(location), None);
            } else {
                // the removed item stays where the added one would be
                self.set_present(&added_location, false)?;
            }
            return Ok(());
        }
        self.set_present(&location, false)?;
        let node = self.insert(OpKind::Remove, location, None, None);
        self.store_index(mkey, node, ST_REMOVE);
        Ok(())
    }

    fn key_part(k: &Bound<'py, PyAny>) -> R<Part<'py>> {
        let s = match k.cast_exact::<PyString>() {
            Ok(s) => s.clone(),
            Err(_) => k.str()?,
        };
        s.to_str().map_err(|_| Fail::Unsupported)?;
        Ok(Part::Key(s))
    }

    fn compare_dicts(&mut self, src: &Bound<'py, PyDict>, dst: &Bound<'py, PyDict>) -> R<()> {
        if self.path.len() > MAX_DEPTH {
            return Err(Fail::Unsupported);
        }
        // Common case: same keys in the same order, so nothing is added or removed
        // and the intersection is just the keys in order.
        if src.len() == dst.len() && same_keys_in_order(src, dst)? {
            for ((k, v), (_, w)) in src.iter().zip(dst.iter()) {
                self.compare_values(Some(Self::key_part(&k)?), &v, &w)?;
            }
            return Ok(());
        }
        for (k, v) in src.iter() {
            if !dst.contains(&k)? {
                self.item_removed(Self::key_part(&k)?, v)?;
            }
        }
        for (k, v) in dst.iter() {
            if !src.contains(&k)? {
                self.item_added(Self::key_part(&k)?, v)?;
            }
        }
        for (k, v) in src.iter() {
            if let Some(w) = dst.get_item(&k)? {
                self.compare_values(Some(Self::key_part(&k)?), &v, &w)?;
            }
        }
        Ok(())
    }

    /// `_differing_runs`
    fn differing_runs(src: &[Bound<'py, PyAny>], dst: &[Bound<'py, PyAny>], ticker: &mut Ticker) -> R<Vec<(usize, usize, usize, usize)>> {
        // Only what is between the common start and end is matched up
        let shorter = src.len().min(dst.len());
        let mut start = 0;
        while start < shorter && same_json(&src[start], &dst[start], 0)? {
            start += 1;
        }
        let mut common_end = 0;
        while common_end < shorter - start && same_json(&src[src.len() - 1 - common_end], &dst[dst.len() - 1 - common_end], 0)? {
            common_end += 1;
        }
        let (src_end, dst_end) = (src.len() - common_end, dst.len() - common_end);

        // Equal ids for items with equal move keys
        let rs = ahash::RandomState::with_seeds(1, 2, 3, 4);
        let mut buckets = AHashMap::new();
        let mut n_ids = 0;
        let mid_src = intern(&src[start..src_end], &mut buckets, &mut n_ids, &rs, ticker)?;
        let mid_dst = intern(&dst[start..dst_end], &mut buckets, &mut n_ids, &rs, ticker)?;
        let n_ids = n_ids as usize;
        // keys by index into src/dst; only the middle is ever read
        let src_key = |i: usize| mid_src[i - start];
        let dst_key = |j: usize| mid_dst[j - start];

        let cost = |(i1, i2, j1, j2): (usize, usize, usize, usize)| -> PyResult<u64> {
            let mut c = (i2 - i1).abs_diff(j2 - j1) as u64;
            for (i, j) in (i1..i2).zip(j1..j2) {
                if src_key(i) != dst_key(j) {
                    c += positional_cost(&src[i], &dst[j])?;
                }
            }
            Ok(c)
        };

        let by_position = (start, src_end, start, dst_end);
        let position_cost = cost(by_position)?;

        let exact = ((src_end - start) as u128) * ((dst_end - start) as u128) <= EXACT_MATCH_LIMIT;
        if !exact {
            let mut counts = vec![0u64; n_ids];
            for &k in &mid_dst {
                counts[k as usize] += 1;
            }
            let comparisons: u128 = mid_src.iter().map(|&k| counts[k as usize] as u128).sum();
            if comparisons > EXACT_MATCH_LIMIT.max((position_cost as u128).pow(2)) {
                return Ok(vec![by_position]);
            }
        }

        let runs: Vec<_> = differing_opcodes(&mid_src, &mid_dst, n_ids, !exact, ticker)?
            .into_iter()
            .map(|(i1, i2, j1, j2)| (start + i1, start + i2, start + j1, start + j2))
            .collect();

        let mut total = 0;
        for &run in &runs {
            total += cost(run)?;
        }
        if total > position_cost {
            return Ok(vec![by_position]);
        }
        Ok(runs)
    }

    fn compare_lists(&mut self, src: &Bound<'py, PyList>, dst: &Bound<'py, PyList>) -> R<()> {
        if self.path.len() > MAX_DEPTH {
            return Err(Fail::Unsupported);
        }
        let src: Vec<_> = src.iter().collect();
        let dst: Vec<_> = dst.iter().collect();
        // The list has become dst[:j1] + src[i1:] when a run is compared, so src[i1]
        // is at index j1
        for (i1, i2, j1, j2) in Self::differing_runs(&src, &dst, &mut self.ticker)? {
            let paired = (i2 - i1).min(j2 - j1);
            for key in j1..j1 + paired {
                self.ticker.tick()?;
                let (old, new) = (&src[i1 + key - j1], &dst[key]);
                match (kind(old), kind(new)) {
                    (Kind::Dict, Kind::Dict) => {
                        self.path.push(Part::Idx(key as i64));
                        self.compare_dicts(old.cast()?, new.cast()?)?;
                        self.path.pop();
                    }
                    (Kind::List, Kind::List) => {
                        self.path.push(Part::Idx(key as i64));
                        self.compare_lists(old.cast()?, new.cast()?)?;
                        self.path.pop();
                    }
                    _ if json_eq(old, new, self.path.len())? => {}
                    _ => {
                        self.item_removed(Part::Idx(key as i64), old.clone())?;
                        self.item_added(Part::Idx(key as i64), new.clone())?;
                    }
                }
            }
            for old in &src[i1 + paired..i2] {
                self.item_removed(Part::Idx((j1 + paired) as i64), old.clone())?;
            }
            for key in j1 + paired..j2 {
                self.item_added(Part::Idx(key as i64), dst[key].clone())?;
            }
        }
        Ok(())
    }

    fn compare_values(&mut self, key: Option<Part<'py>>, src: &Bound<'py, PyAny>, dst: &Bound<'py, PyAny>) -> R<()> {
        let pushed = key.is_some();
        match (kind(src), kind(dst)) {
            (Kind::Dict, Kind::Dict) | (Kind::List, Kind::List) => {
                if let Some(k) = key {
                    self.path.push(k);
                }
                let r = if let Ok(s) = src.cast::<PyDict>() {
                    self.compare_dicts(s, dst.cast()?)
                } else {
                    self.compare_lists(src.cast()?, dst.cast()?)
                };
                if pushed {
                    self.path.pop();
                }
                r
            }
            _ if json_eq(src, dst, self.path.len())? => Ok(()),
            _ => {
                let parts = match key {
                    Some(k) => self.joined(k),
                    None => self.path.clone(),
                };
                let loc = self.location(&parts);
                self.insert(OpKind::Replace, loc, None, Some(dst.clone()));
                Ok(())
            }
        }
    }

    /// `_replay`: the operations with the array indices at which they apply.
    fn replay(&mut self) -> R<Vec<(OpKind, Path<'py>, Option<Path<'py>>, Option<Bound<'py, PyAny>>)>> {
        for a in 0..self.array_data.len() {
            let present: Vec<usize> = self.array_data[a].items.iter().copied().filter(|&i| self.items[i].initial).collect();
            self.array_data[a].present_labels = present.iter().map(|&i| self.items[i].label).collect();
            self.array_data[a].present = present;
        }
        let mut out = vec![];
        let mut curr = self.ops[0].next;
        while curr != 0 {
            self.ticker.tick()?;
            // 'from' is removed before 'path' is added
            let from = self.ops[curr].from.clone();
            let from_parts = match &from {
                Some(f) => {
                    let p = self.parts(f);
                    self.set_present(f, false)?;
                    Some(p)
                }
                None => None,
            };
            let path = self.ops[curr].path.clone();
            let parts = self.parts(&path);
            match self.ops[curr].kind {
                OpKind::Remove => self.set_present(&path, false)?,
                OpKind::Add | OpKind::Move => self.set_present(&path, true)?,
                OpKind::Replace => {}
            }
            out.push((self.ops[curr].kind, parts, from_parts, self.ops[curr].value.clone()));
            curr = self.ops[curr].next;
        }
        Ok(out)
    }

    /// See `DiffBuilder.execute`: emit operation dicts, merging remove+add into replace.
    fn execute(&mut self, py: Python<'py>) -> R<Bound<'py, PyList>> {
        let ops = self.replay()?;
        let out = PyList::empty(py);
        let mut i = 0;
        while i < ops.len() {
            self.ticker.tick()?;
            let d = PyDict::new(py);
            let (kind, path, from, value) = &ops[i];
            if let Some((OpKind::Add, path2, _, value2)) = ops.get(i + 1) {
                if *kind == OpKind::Remove && path == path2 {
                    d.set_item(intern!(py, "op"), intern!(py, "replace"))?;
                    d.set_item(intern!(py, "path"), to_pointer(path2))?;
                    d.set_item(intern!(py, "value"), value2.as_ref().unwrap())?;
                    out.append(d)?;
                    i += 2;
                    continue;
                }
            }
            let name = match kind {
                OpKind::Add => intern!(py, "add"),
                OpKind::Remove => intern!(py, "remove"),
                OpKind::Replace => intern!(py, "replace"),
                OpKind::Move => intern!(py, "move"),
            };
            d.set_item(intern!(py, "op"), name)?;
            if let Some(from) = from {
                d.set_item(intern!(py, "from"), to_pointer(from))?;
            }
            d.set_item(intern!(py, "path"), to_pointer(path))?;
            if let Some(v) = value {
                d.set_item(intern!(py, "value"), v)?;
            }
            out.append(d)?;
            i += 1;
        }
        Ok(out)
    }
}

/// Operations turning `src` into `dst`, as `jsonpatch.make_patch(src, dst).patch`
/// would produce them; `None` when the reference implementation must be used.
#[pyfunction]
fn diff<'py>(py: Python<'py>, src: &Bound<'py, PyAny>, dst: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyList>>> {
    let mut b = Builder::new(py);
    match b.compare_values(None, src, dst).and_then(|_| b.execute(py)) {
        Ok(ops) => Ok(Some(ops)),
        Err(Fail::Unsupported) => Ok(None),
        Err(Fail::Py(e)) => Err(e),
    }
}

// ---------------------------------------------------------------- deepcopy

struct Copier<'py> {
    py: Python<'py>,
    memo: AHashMap<usize, Bound<'py, PyAny>>,
}

impl<'py> Copier<'py> {
    /// `copy.deepcopy` for JSON-shaped data; other objects go to `copy.deepcopy`.
    fn copy(&mut self, x: &Bound<'py, PyAny>, depth: usize) -> PyResult<Bound<'py, PyAny>> {
        let k = kind(x);
        if matches!(k, Kind::Str | Kind::Int | Kind::Float | Kind::Bool | Kind::Null) {
            return Ok(x.clone());
        }
        if k == Kind::Other || depth > MAX_DEPTH {
            return self.py.import(intern!(self.py, "copy"))?.call_method1(intern!(self.py, "deepcopy"), (x,));
        }
        // A container referenced only by its parent cannot be reached twice, so it needs
        // no memo entry. Below the root, the other references are the parent's shallow
        // copy and the iterator's.
        let id = x.as_ptr() as usize;
        let shared = depth == 0 || unsafe { ffi::Py_REFCNT(x.as_ptr()) } > 3;
        if shared {
            if let Some(y) = self.memo.get(&id) {
                return Ok(y.clone());
            }
        }
        // Shallow-copy in one C call, then replace the values that are not atomic.
        if k == Kind::Dict {
            let d = x.cast::<PyDict>()?;
            let mut pos = 0;
            while let Some((key, _)) = dict_next(d, &mut pos) {
                // SAFETY: borrowed from `d`, which outlives this check.
                if kind(&*unsafe { Borrowed::from_ptr(self.py, key) }) == Kind::Other {
                    return self.py.import(intern!(self.py, "copy"))?.call_method1(intern!(self.py, "deepcopy"), (x,));
                }
            }
            let y = d.copy()?;
            if shared {
                self.memo.insert(id, y.clone().into_any());
            }
            for (key, v) in d.iter() {
                if matches!(kind(&v), Kind::Dict | Kind::List | Kind::Other) {
                    y.set_item(key, self.copy(&v, depth + 1)?)?;
                }
            }
            Ok(y.into_any())
        } else {
            let l = x.cast::<PyList>()?;
            let y = l.get_slice(0, l.len()).cast_into::<PyList>()?;
            if shared {
                self.memo.insert(id, y.clone().into_any());
            }
            for (i, v) in l.iter().enumerate() {
                if matches!(kind(&v), Kind::Dict | Kind::List | Kind::Other) {
                    y.set_item(i, self.copy(&v, depth + 1)?)?;
                }
            }
            Ok(y.into_any())
        }
    }
}

fn deepcopy<'py>(x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    Copier { py: x.py(), memo: AHashMap::new() }.copy(x, 0)
}

#[pyfunction(name = "deepcopy")]
fn py_deepcopy<'py>(x: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    deepcopy(x)
}

// ---------------------------------------------------------------- apply

#[derive(Clone, Copy, PartialEq)]
enum Verb {
    Add,
    Remove,
    Replace,
    Move,
    Test,
    Copy,
}

/// A JSON pointer as jsonpointer parses it; `None` if jsonpointer would reject it.
fn parse_pointer(s: &str) -> Option<Vec<String>> {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'~' && !matches!(b.get(i + 1), Some(b'0' | b'1')) {
            return None;
        }
    }
    if s.is_empty() {
        return Some(vec![]);
    }
    let rest = s.strip_prefix('/')?;
    Some(rest.split('/').map(|p| p.replace("~1", "/").replace("~0", "~")).collect())
}

/// The verb and path of an operation in plain form, or `None` for jsonpatch to handle.
fn fast_op(op: &Bound<'_, PyAny>) -> PyResult<Option<(Verb, Vec<String>)>> {
    let Ok(d) = op.cast_exact::<PyDict>() else { return Ok(None) };
    let py = op.py();
    let Some(name) = d.get_item(intern!(py, "op"))? else { return Ok(None) };
    let Ok(name) = name.cast_exact::<PyString>() else { return Ok(None) };
    let verb = match name.to_str() {
        Ok("add") => Verb::Add,
        Ok("remove") => Verb::Remove,
        Ok("replace") => Verb::Replace,
        Ok("move") => Verb::Move,
        Ok("test") => Verb::Test,
        Ok("copy") => Verb::Copy,
        _ => return Ok(None),
    };
    Ok(str_pointer(d, intern!(py, "path"))?.map(|p| (verb, p)))
}

fn str_pointer(d: &Bound<'_, PyDict>, member: &Bound<'_, PyString>) -> PyResult<Option<Vec<String>>> {
    let Some(path) = d.get_item(member)? else { return Ok(None) };
    let Ok(path) = path.cast_exact::<PyString>() else { return Ok(None) };
    Ok(path.to_str().ok().and_then(parse_pointer))
}

/// Array index as jsonpointer accepts it (`0|[1-9][0-9]*`).
fn array_index(part: &str) -> Option<usize> {
    let b = part.as_bytes();
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) || (b[0] == b'0' && b.len() > 1) {
        return None;
    }
    part.parse().ok()
}

/// Walk all but the last part through exact dicts and lists.
fn parent<'py>(doc: &Bound<'py, PyAny>, parts: &[String]) -> PyResult<Option<Bound<'py, PyAny>>> {
    let mut cur = doc.clone();
    for part in parts {
        cur = match kind(&cur) {
            Kind::Dict => match cur.cast::<PyDict>()?.get_item(part)? {
                Some(v) => v,
                None => return Ok(None),
            },
            Kind::List => {
                let l = cur.cast::<PyList>()?;
                match array_index(part) {
                    Some(i) if i < l.len() => l.get_item(i)?,
                    _ => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
    }
    Ok(Some(cur))
}

enum Slot<'py> {
    Key(Bound<'py, PyDict>, String),
    /// index (may equal or exceed len), or `None` for "-"
    Idx(Bound<'py, PyList>, Option<usize>),
}

/// Resolve the container and last step of a non-empty path.
fn slot<'py>(doc: &Bound<'py, PyAny>, parts: &[String]) -> PyResult<Option<Slot<'py>>> {
    let (last, init) = parts.split_last().unwrap();
    let Some(p) = parent(doc, init)? else { return Ok(None) };
    Ok(match kind(&p) {
        Kind::Dict => Some(Slot::Key(p.cast_into()?, last.clone())),
        Kind::List if last == "-" => Some(Slot::Idx(p.cast_into()?, None)),
        Kind::List => array_index(last).map(|i| Slot::Idx(p.cast_into().unwrap(), Some(i))),
        _ => None,
    })
}

/// Value at a slot that exists, as `subobj[part]` would find it.
fn slot_value<'py>(s: &Slot<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
    match s {
        Slot::Key(d, k) => d.get_item(k),
        Slot::Idx(l, Some(i)) if *i < l.len() => Ok(Some(l.get_item(*i)?)),
        Slot::Idx(..) => Ok(None),
    }
}

/// `AddOperation._add`; `None` when jsonpatch must handle it (nothing was mutated).
fn add_fast<'py>(obj: &Bound<'py, PyAny>, parts: &[String], value: Bound<'py, PyAny>) -> PyResult<Result<Bound<'py, PyAny>, Bound<'py, PyAny>>> {
    if parts.is_empty() {
        return Ok(Ok(value)); // the root is replaced, whatever its type
    }
    match slot(obj, parts)? {
        Some(Slot::Key(d, k)) => d.set_item(k, value)?,
        Some(Slot::Idx(l, None)) => l.append(value)?,
        Some(Slot::Idx(l, Some(i))) if i <= l.len() => l.insert(i, value)?,
        _ => return Ok(Err(value)),
    }
    Ok(Ok(obj.clone()))
}

struct Applier<'a, 'py> {
    get_operation: &'a Bound<'py, PyAny>,
    add_stage: &'a Bound<'py, PyAny>,
}

impl<'py> Applier<'_, 'py> {
    fn delegate(&self, op: &Bound<'py, PyAny>, obj: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        self.get_operation.call1((op,))?.call_method1(intern!(op.py(), "apply"), (obj,))
    }

    fn add(&self, obj: &Bound<'py, PyAny>, op: &Bound<'py, PyDict>, parts: &[String], value: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        match add_fast(obj, parts, value)? {
            Ok(obj) => Ok(obj),
            Err(value) => {
                let location = op.get_item(intern!(op.py(), "path"))?.unwrap();
                self.add_stage.call1((obj, location, value))
            }
        }
    }

    /// Apply one operation; `Ok(None)` means "not handled, nothing mutated".
    fn try_apply(&self, obj: &Bound<'py, PyAny>, op: &Bound<'py, PyAny>, verb: Verb, parts: &[String]) -> PyResult<Option<Bound<'py, PyAny>>> {
        let py = op.py();
        let d = op.cast::<PyDict>()?;
        let value = || d.get_item(intern!(py, "value"));
        match verb {
            Verb::Add => {
                let Some(v) = value()? else { return Ok(None) };
                let v = deepcopy(&v)?;
                Ok(add_fast(obj, parts, v)?.ok())
            }
            Verb::Replace => {
                let Some(v) = value()? else { return Ok(None) };
                if parts.is_empty() {
                    return Ok(Some(deepcopy(&v)?));
                }
                let Some(s) = slot(obj, parts)? else { return Ok(None) };
                if slot_value(&s)?.is_none() {
                    return Ok(None);
                }
                let v = deepcopy(&v)?;
                match s {
                    Slot::Key(d, k) => d.set_item(k, v)?,
                    Slot::Idx(l, Some(i)) => l.set_item(i, v)?,
                    Slot::Idx(_, None) => unreachable!(),
                }
                Ok(Some(obj.clone()))
            }
            Verb::Remove => {
                if parts.is_empty() {
                    return Ok(None);
                }
                let Some(s) = slot(obj, parts)? else { return Ok(None) };
                if slot_value(&s)?.is_none() {
                    return Ok(None);
                }
                remove_slot(s)?;
                Ok(Some(obj.clone()))
            }
            Verb::Test => {
                let actual = if parts.is_empty() {
                    obj.clone()
                } else {
                    let Some(s) = slot(obj, parts)? else { return Ok(None) };
                    let Some(v) = slot_value(&s)? else { return Ok(None) };
                    v
                };
                let Some(expected) = value()? else { return Ok(None) };
                Ok(if json_equal(&actual, &expected, 0)? { Some(obj.clone()) } else { None })
            }
            Verb::Move => {
                let Some(from) = str_pointer(d, intern!(py, "from"))? else { return Ok(None) };
                if from.is_empty() {
                    return Ok(None);
                }
                let Some(s) = slot(obj, &from)? else { return Ok(None) };
                let Some(v) = slot_value(&s)? else { return Ok(None) };
                if from == parts {
                    return Ok(Some(obj.clone()));
                }
                if parts.starts_with(&from) {
                    return Ok(None); // "Cannot move values into their own children"
                }
                remove_slot(s)?;
                Ok(Some(self.add(obj, d, parts, v)?))
            }
            Verb::Copy => {
                let Some(from) = str_pointer(d, intern!(py, "from"))? else { return Ok(None) };
                let v = if from.is_empty() {
                    obj.clone()
                } else {
                    let Some(s) = slot(obj, &from)? else { return Ok(None) };
                    let Some(v) = slot_value(&s)? else { return Ok(None) };
                    v
                };
                let v = deepcopy(&v)?;
                Ok(Some(self.add(obj, d, parts, v)?))
            }
        }
    }
}

/// `_json_equal` (bools equal only bools) for dicts, lists and scalars; false for
/// anything else, which jsonpatch then compares.
fn json_equal(a: &Bound<'_, PyAny>, b: &Bound<'_, PyAny>, depth: usize) -> PyResult<bool> {
    if depth > MAX_DEPTH {
        return Ok(false);
    }
    match (kind(a), kind(b)) {
        (Kind::Bool, Kind::Bool) => Ok(a.is(b)),
        (Kind::Bool, _) | (_, Kind::Bool) => Ok(false),
        (Kind::Dict, Kind::Dict) => {
            let (x, y) = (a.cast::<PyDict>()?, b.cast::<PyDict>()?);
            if x.len() != y.len() {
                return Ok(false);
            }
            for (k, v) in x.iter() {
                match y.get_item(&k)? {
                    Some(w) if json_equal(&v, &w, depth + 1)? => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        (Kind::List, Kind::List) => {
            let (x, y) = (a.cast::<PyList>()?, b.cast::<PyList>()?);
            if x.len() != y.len() {
                return Ok(false);
            }
            for (v, w) in x.iter().zip(y.iter()) {
                if !json_equal(&v, &w, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Kind::Dict | Kind::List | Kind::Other, _) | (_, Kind::Dict | Kind::List | Kind::Other) => Ok(false),
        _ => a.eq(b),
    }
}

fn remove_slot(s: Slot) -> PyResult<()> {
    match s {
        Slot::Key(d, k) => d.del_item(k),
        Slot::Idx(l, Some(i)) => l.del_item(i),
        Slot::Idx(_, None) => unreachable!(),
    }
}

fn items<'py>(patch: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyAny>>> {
    patch.try_iter()?.collect()
}

/// Check every operation like `JsonPatch.__init__`; unusual ones go to `check(op)`.
#[pyfunction]
fn validate(patch: &Bound<'_, PyAny>, check: &Bound<'_, PyAny>) -> PyResult<()> {
    let mut ticker = Ticker::new(patch.py());
    for op in items(patch)? {
        ticker.tick()?;
        if fast_op(&op)?.is_none() {
            check.call1((&op,))?;
        }
    }
    Ok(())
}

/// `JsonPatch.apply` for the default operation classes and pointer class.
#[pyfunction]
fn apply<'py>(
    patch: &Bound<'py, PyAny>,
    obj: &Bound<'py, PyAny>,
    in_place: bool,
    get_operation: &Bound<'py, PyAny>,
    add_stage: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let ops = items(patch)?;
    let mut plans = Vec::with_capacity(ops.len());
    let mut ticker = Ticker::new(patch.py());
    for op in &ops {
        ticker.tick()?;
        let plan = fast_op(op)?;
        if plan.is_none() {
            get_operation.call1((op,))?; // raises like `JsonPatch._ops` would
        }
        plans.push(plan);
    }
    let mut obj = if in_place { obj.clone() } else { deepcopy(obj)? };
    let a = Applier { get_operation, add_stage };
    for (op, plan) in ops.iter().zip(plans) {
        ticker.tick()?;
        obj = match plan {
            Some((verb, parts)) => match a.try_apply(&obj, op, verb, &parts)? {
                Some(o) => o,
                None => a.delegate(op, &obj)?,
            },
            None => a.delegate(op, &obj)?,
        };
    }
    Ok(obj)
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(apply, m)?)?;
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(py_deepcopy, m)?)?;
    // The package version, so `fastjsonpatch.__version__` needs no importlib.metadata.
    // Cargo writes pre-releases as semver (1.0.0-rc.1), PyPI as PEP 440 (1.0.0rc1).
    let version = env!("CARGO_PKG_VERSION")
        .replace("-alpha.", "a")
        .replace("-beta.", "b")
        .replace("-rc.", "rc");
    m.add("__version__", version)?;
    Ok(())
}
