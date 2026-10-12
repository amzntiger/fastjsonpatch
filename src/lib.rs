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
use pyo3::exceptions::PyTypeError;
use pyo3::ffi;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

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

/// `json.dumps(a) == json.dumps(b)` for values that are not both dicts or both lists.
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
    if ka != kb {
        return Ok(false); // e.g. 1 vs 1.0 vs true: "1", "1.0", "true"
    }
    Ok(match ka {
        Kind::Float => {
            let (x, y) = (a.cast::<PyFloat>()?.value(), b.cast::<PyFloat>()?.value());
            (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits()
        }
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

#[derive(Clone)]
enum Part<'py> {
    Key(Bound<'py, PyString>),
    Idx(i64),
}

impl PartialEq for Part<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Part::Idx(a), Part::Idx(b)) => a == b,
            (Part::Key(a), Part::Key(b)) => {
                a.is(b)
                    || match (a.to_str(), b.to_str()) {
                        (Ok(x), Ok(y)) => x == y,
                        _ => a.as_any().eq(b).unwrap_or(false),
                    }
            }
            _ => false,
        }
    }
}

type Path<'py> = Vec<Part<'py>>;

fn is_int(p: &Part) -> bool {
    matches!(p, Part::Idx(_))
}

fn idx(p: &Part) -> i64 {
    match p {
        Part::Idx(i) => *i,
        Part::Key(_) => i64::MIN, // unreachable for paths produced by the diff
    }
}

fn is_prefix(sub: &[Part], parts: &[Part]) -> bool {
    sub.len() <= parts.len() && sub == &parts[..sub.len()]
}

fn is_inside(parts: &[Part], container: &[Part]) -> bool {
    parts.len() > container.len() && is_prefix(container, parts)
}

fn shift<'py>(parts: &[Part<'py>], depth: usize, offset: i64) -> Path<'py> {
    let mut p = parts.to_vec();
    p[depth] = Part::Idx(idx(&p[depth]) + offset);
    p
}

/// `parts` assumes no item at `location`; adjust it to the item being there.
fn with_item<'py>(parts: &[Part<'py>], location: &[Part<'py>], insertion: bool) -> Path<'py> {
    let depth = location.len() - 1;
    if is_int(&location[depth]) && is_inside(parts, &location[..depth]) {
        let (index, loc) = (idx(&parts[depth]), idx(&location[depth]));
        let before_item = insertion && parts.len() == location.len() && index == loc;
        if index >= loc && !before_item {
            return shift(parts, depth, 1);
        }
    }
    parts.to_vec()
}

/// `parts` assumes an item at `location`; adjust it to the item missing.
fn without_item<'py>(parts: &[Part<'py>], location: &[Part<'py>]) -> Path<'py> {
    let depth = location.len() - 1;
    if is_int(&location[depth]) && is_inside(parts, &location[..depth]) && idx(&parts[depth]) > idx(&location[depth]) {
        return shift(parts, depth, -1);
    }
    parts.to_vec()
}

/// Where the item at `location` is after inserting (or removing) the value at `parts`.
fn item_after<'py>(location: &[Part<'py>], parts: &[Part<'py>], inserted: bool) -> Path<'py> {
    let depth = parts.len() - 1;
    if !is_int(&parts[depth]) || !is_inside(location, &parts[..depth]) {
        return location.to_vec();
    }
    let (at, p) = (idx(&location[depth]), idx(&parts[depth]));
    if inserted && at >= p {
        return shift(location, depth, 1);
    }
    if !inserted && at > p {
        return shift(location, depth, -1);
    }
    location.to_vec()
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

fn to_pointer(path: &[Part]) -> R<String> {
    let mut s = String::new();
    for p in path {
        s.push('/');
        match p {
            Part::Idx(i) => s.push_str(&i.to_string()),
            Part::Key(k) => escape_into(&mut s, k.to_str().map_err(|_| Fail::Unsupported)?),
        }
    }
    Ok(s)
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
    path: Path<'py>,
    from: Option<Path<'py>>,
    value: Option<Bound<'py, PyAny>>,
    prev: usize,
    next: usize,
}

const ST_ADD: usize = 0;
const ST_REMOVE: usize = 1;

struct Builder<'py> {
    py: Python<'py>,
    /// doubly linked list of operations; node 0 is the sentinel
    ops: Vec<Op<'py>>,
    /// `(value, type(value))` -> slot in `slots`, for hashable values
    storage: [Bound<'py, PyDict>; 2],
    slots: [Vec<Vec<usize>>; 2],
    /// unhashable values, searched linearly like jsonpatch does
    storage2: [Vec<(Bound<'py, PyAny>, usize)>; 2],
    path: Path<'py>,
}

impl<'py> Builder<'py> {
    fn new(py: Python<'py>) -> Self {
        let root = Op { kind: OpKind::Add, path: vec![], from: None, value: None, prev: 0, next: 0 };
        Builder {
            py,
            ops: vec![root],
            storage: [PyDict::new(py), PyDict::new(py)],
            slots: [vec![], vec![]],
            storage2: [vec![], vec![]],
            path: vec![],
        }
    }

    fn typed_key(&self, value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        Ok(PyTuple::new(self.py, [value.clone(), value.get_type().into_any()])?.into_any())
    }

    fn store_index(&mut self, value: &Bound<'py, PyAny>, node: usize, st: usize) -> PyResult<()> {
        let key = self.typed_key(value)?;
        match self.storage[st].get_item(&key) {
            Ok(Some(slot)) => self.slots[st][slot.extract::<usize>()?].push(node),
            Ok(None) => {
                self.storage[st].set_item(&key, self.slots[st].len())?;
                self.slots[st].push(vec![node]);
            }
            Err(e) if e.is_instance_of::<PyTypeError>(self.py) => self.storage2[st].push((key, node)),
            Err(e) => return Err(e),
        }
        Ok(())
    }

    fn take_index(&mut self, value: &Bound<'py, PyAny>, st: usize) -> PyResult<Option<usize>> {
        let key = self.typed_key(value)?;
        match self.storage[st].get_item(&key) {
            Ok(Some(slot)) => Ok(self.slots[st][slot.extract::<usize>()?].pop()),
            Ok(None) => Ok(None),
            Err(e) if e.is_instance_of::<PyTypeError>(self.py) => {
                for i in (0..self.storage2[st].len()).rev() {
                    if self.storage2[st][i].0.eq(&key)? {
                        return Ok(Some(self.storage2[st].remove(i).1));
                    }
                }
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn insert(&mut self, kind: OpKind, path: Path<'py>, from: Option<Path<'py>>, value: Option<Bound<'py, PyAny>>) -> usize {
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

    /// See `DiffBuilder._adjust_following`. Changes are `(node, is_from, new path)`.
    fn adjust_following(&self, index: usize, removed: bool) -> (Vec<(usize, bool, Path<'py>)>, Path<'py>) {
        let mut location = self.ops[index].path.clone();
        let mut changes = vec![];
        if !location.iter().any(is_int) {
            return (changes, location);
        }
        let mut curr = self.ops[index].next;
        while curr != 0 {
            let op = &self.ops[curr];
            for is_from in [true, false] {
                let member = if is_from {
                    match &op.from {
                        Some(f) => f,
                        None => continue,
                    }
                } else {
                    &op.path
                };
                let inserted = !is_from && matches!(op.kind, OpKind::Add | OpKind::Move);
                let (parts, present) = if removed {
                    let p = with_item(member, &location, inserted);
                    (p.clone(), p)
                } else {
                    (without_item(member, &location), member.clone())
                };
                if op.kind != OpKind::Replace {
                    location = item_after(&location, &present, inserted);
                }
                changes.push((curr, is_from, parts));
            }
            curr = op.next;
        }
        (changes, location)
    }

    fn apply_changes(&mut self, changes: Vec<(usize, bool, Path<'py>)>) {
        for (node, is_from, parts) in changes {
            if is_from {
                self.ops[node].from = Some(parts);
            } else {
                self.ops[node].path = parts;
            }
        }
    }

    fn joined(&self, key: Part<'py>) -> Path<'py> {
        let mut p = self.path.clone();
        p.push(key);
        p
    }

    fn item_added(&mut self, key: Part<'py>, item: Bound<'py, PyAny>) -> R<()> {
        let target = self.joined(key);
        if let Some(index) = self.take_index(&item, ST_REMOVE)? {
            let (changes, source) = self.adjust_following(index, true);
            if !is_inside(&target, &source) {
                self.apply_changes(changes);
                self.remove(index);
                if source != target {
                    self.insert(OpKind::Move, target, Some(source), None);
                }
                return Ok(());
            }
        }
        let node = self.insert(OpKind::Add, target, None, Some(item.clone()));
        self.store_index(&item, node, ST_ADD)?;
        Ok(())
    }

    fn item_removed(&mut self, key: Part<'py>, item: Bound<'py, PyAny>) -> R<()> {
        let source = self.joined(key);
        if let Some(index) = self.take_index(&item, ST_ADD)? {
            let (changes, added) = self.adjust_following(index, false);
            let moved_from = without_item(&source, &added);
            let target = item_after(&added, &source, false);
            if !is_inside(&target, &moved_from) {
                self.apply_changes(changes);
                self.remove(index);
                if moved_from != target {
                    self.insert(OpKind::Move, target, Some(moved_from), None);
                }
                return Ok(());
            }
        }
        let node = self.insert(OpKind::Remove, source, None, None);
        self.store_index(&item, node, ST_REMOVE)?;
        Ok(())
    }

    fn key_part(k: &Bound<'py, PyAny>) -> PyResult<Part<'py>> {
        Ok(Part::Key(match k.cast_exact::<PyString>() {
            Ok(s) => s.clone(),
            Err(_) => k.str()?,
        }))
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

    fn compare_lists(&mut self, src: &Bound<'py, PyList>, dst: &Bound<'py, PyList>) -> R<()> {
        if self.path.len() > MAX_DEPTH {
            return Err(Fail::Unsupported);
        }
        let (len_src, len_dst) = (src.len(), dst.len());
        for key in 0..len_src.max(len_dst) {
            if key < len_src.min(len_dst) {
                let (old, new) = (src.get_item(key)?, dst.get_item(key)?);
                match (kind(&old), kind(&new)) {
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
                    _ if json_eq(&old, &new, self.path.len())? => {}
                    _ => {
                        self.item_removed(Part::Idx(key as i64), old)?;
                        self.item_added(Part::Idx(key as i64), new)?;
                    }
                }
            } else if len_src > len_dst {
                self.item_removed(Part::Idx(len_dst as i64), src.get_item(key)?)?;
            } else {
                self.item_added(Part::Idx(key as i64), dst.get_item(key)?)?;
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
                let path = match key {
                    Some(k) => self.joined(k),
                    None => self.path.clone(),
                };
                self.insert(OpKind::Replace, path, None, Some(dst.clone()));
                Ok(())
            }
        }
    }

    /// See `DiffBuilder.execute`: emit operation dicts, merging remove+add into replace.
    fn execute(&self) -> R<Bound<'py, PyList>> {
        let py = self.py;
        let out = PyList::empty(py);
        let mut curr = self.ops[0].next;
        while curr != 0 {
            let op = &self.ops[curr];
            let d = PyDict::new(py);
            let next = op.next;
            if next != 0 {
                let second = &self.ops[next];
                if op.kind == OpKind::Remove && second.kind == OpKind::Add && op.path == second.path {
                    d.set_item(intern!(py, "op"), intern!(py, "replace"))?;
                    d.set_item(intern!(py, "path"), to_pointer(&second.path)?)?;
                    d.set_item(intern!(py, "value"), second.value.as_ref().unwrap())?;
                    out.append(d)?;
                    curr = second.next;
                    continue;
                }
            }
            let name = match op.kind {
                OpKind::Add => intern!(py, "add"),
                OpKind::Remove => intern!(py, "remove"),
                OpKind::Replace => intern!(py, "replace"),
                OpKind::Move => intern!(py, "move"),
            };
            d.set_item(intern!(py, "op"), name)?;
            if let Some(from) = &op.from {
                d.set_item(intern!(py, "from"), to_pointer(from)?)?;
            }
            d.set_item(intern!(py, "path"), to_pointer(&op.path)?)?;
            if let Some(v) = &op.value {
                d.set_item(intern!(py, "value"), v)?;
            }
            out.append(d)?;
            curr = next;
        }
        Ok(out)
    }
}

/// Operations turning `src` into `dst`, as `jsonpatch.make_patch(src, dst).patch`
/// would produce them; `None` when the reference implementation must be used.
#[pyfunction]
fn diff<'py>(py: Python<'py>, src: &Bound<'py, PyAny>, dst: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyList>>> {
    let mut b = Builder::new(py);
    match b.compare_values(None, src, dst).and_then(|_| b.execute()) {
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
        return Ok(if matches!(kind(obj), Kind::Dict | Kind::List) { Ok(value) } else { Err(value) });
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
                if parts.last().is_some_and(|p| p == "-") {
                    return Ok(None); // jsonpatch rejects "-" here even for a dict key
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
                Ok(if actual.ne(&expected)? { None } else { Some(obj.clone()) })
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
                if matches!(s, Slot::Key(..)) && parts.starts_with(&from) {
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
    for op in items(patch)? {
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
    for op in &ops {
        let plan = fast_op(op)?;
        if plan.is_none() {
            get_operation.call1((op,))?; // raises like `JsonPatch._ops` would
        }
        plans.push(plan);
    }
    let mut obj = if in_place { obj.clone() } else { deepcopy(obj)? };
    let a = Applier { get_operation, add_stage };
    for (op, plan) in ops.iter().zip(plans) {
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
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
