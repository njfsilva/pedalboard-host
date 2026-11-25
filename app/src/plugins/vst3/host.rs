//! Objects the host hands to VST3 plugins: the host context (which also makes the messages a
//! plugin's two halves use to talk), the edit handler, the editor frame, memory streams, and the
//! preallocated parameter and event lists used inside the audio callback.

use super::com::*;
use crate::plugins::editor::resize_client;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, MutexGuard};

fn is(iid: *const Tuid, want: &Tuid) -> bool {
    !iid.is_null() && unsafe { *iid == *want }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reference counting for objects the host owns outright.
unsafe extern "system" fn no_ref(_this: *mut c_void) -> u32 {
    1
}

unsafe fn self_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void, own: &Tuid) -> TResult {
    if obj.is_null() {
        return INVALID_ARG;
    }
    if is(iid, &IID_FUNKNOWN) || is(iid, own) {
        *obj = this;
        return OK;
    }
    *obj = null_mut();
    NO_INTERFACE
}

/// queryInterface, addRef and release for a heap object with a `refs` counter.
macro_rules! com_object {
    ($t:ident, $qi:ident, $add:ident, $rel:ident, $iid:expr) => {
        unsafe extern "system" fn $qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
            let r = self_qi(this, iid, obj, &$iid);
            if r == OK {
                $add(this);
            }
            r
        }
        unsafe extern "system" fn $add(this: *mut c_void) -> u32 {
            (*(this as *const $t)).refs.fetch_add(1, Relaxed) + 1
        }
        unsafe extern "system" fn $rel(this: *mut c_void) -> u32 {
            let left = (*(this as *const $t)).refs.fetch_sub(1, AcqRel) - 1;
            if left == 0 {
                drop(Box::from_raw(this as *mut $t));
            }
            left
        }
    };
}

// ---- Host context -------------------------------------------------------------------------

#[repr(C)]
struct HostApp {
    vtbl: *const HostApplicationVtbl,
}

unsafe impl Sync for HostApp {}

static HOST_VTBL: HostApplicationVtbl = HostApplicationVtbl {
    query_interface: host_qi,
    add_ref: no_ref,
    release: no_ref,
    get_name: host_name,
    create_instance: host_create,
};

static HOST: HostApp = HostApp { vtbl: &HOST_VTBL };

/// The host context passed to the factory and to `initialize`.
pub fn host_context() -> *mut c_void {
    &HOST as *const HostApp as *mut c_void
}

unsafe extern "system" fn host_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    self_qi(this, iid, obj, &IID_HOST_APPLICATION)
}

unsafe extern "system" fn host_name(_this: *mut c_void, name: *mut u16) -> TResult {
    write_wstr(name, 128, "Pedalboard Host");
    OK
}

unsafe extern "system" fn host_create(_this: *mut c_void, cid: *const Tuid, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    if obj.is_null() {
        return INVALID_ARG;
    }
    *obj = null_mut();
    let made = if is(cid, &IID_MESSAGE) {
        new_message()
    } else if is(cid, &IID_ATTRIBUTE_LIST) {
        new_attributes()
    } else {
        return NO_INTERFACE;
    };
    let r = ((**(made as *const *const UnknownVtbl)).query_interface)(made, iid, obj);
    release(made);
    r
}

// ---- Messages between a plugin's processor and controller ----------------------------------

#[repr(C)]
struct Message {
    vtbl: *const MessageVtbl,
    refs: AtomicU32,
    id: Mutex<Option<CString>>,
    attrs: *mut c_void,
}

impl Drop for Message {
    fn drop(&mut self) {
        unsafe { release(self.attrs) };
    }
}

com_object!(Message, msg_qi, msg_add_ref, msg_release, IID_MESSAGE);

static MSG_VTBL: MessageVtbl = MessageVtbl {
    query_interface: msg_qi,
    add_ref: msg_add_ref,
    release: msg_release,
    get_message_id: msg_get_id,
    set_message_id: msg_set_id,
    get_attributes: msg_attributes,
};

fn new_message() -> *mut c_void {
    let m = Message { vtbl: &MSG_VTBL, refs: AtomicU32::new(1), id: Mutex::new(None), attrs: new_attributes() };
    Box::into_raw(Box::new(m)) as *mut c_void
}

unsafe extern "system" fn msg_get_id(this: *mut c_void) -> *const c_char {
    lock(&(*(this as *const Message)).id).as_ref().map_or(null(), |s| s.as_ptr())
}

unsafe extern "system" fn msg_set_id(this: *mut c_void, id: *const c_char) {
    let v = if id.is_null() { None } else { Some(CStr::from_ptr(id).to_owned()) };
    *lock(&(*(this as *const Message)).id) = v;
}

/// Borrowed, as the interface specifies: no reference is added.
unsafe extern "system" fn msg_attributes(this: *mut c_void) -> *mut c_void {
    (*(this as *const Message)).attrs
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[repr(C)]
struct Attributes {
    vtbl: *const AttributeListVtbl,
    refs: AtomicU32,
    map: Mutex<HashMap<Vec<u8>, Attr>>,
}

com_object!(Attributes, attr_qi, attr_add_ref, attr_release, IID_ATTRIBUTE_LIST);

static ATTR_VTBL: AttributeListVtbl = AttributeListVtbl {
    query_interface: attr_qi,
    add_ref: attr_add_ref,
    release: attr_release,
    set_int: attr_set_int,
    get_int: attr_get_int,
    set_float: attr_set_float,
    get_float: attr_get_float,
    set_string: attr_set_string,
    get_string: attr_get_string,
    set_binary: attr_set_binary,
    get_binary: attr_get_binary,
};

fn new_attributes() -> *mut c_void {
    let a = Attributes { vtbl: &ATTR_VTBL, refs: AtomicU32::new(1), map: Mutex::new(HashMap::new()) };
    Box::into_raw(Box::new(a)) as *mut c_void
}

unsafe fn attr_set(this: *mut c_void, id: *const c_char, v: Attr) -> TResult {
    if id.is_null() {
        return INVALID_ARG;
    }
    lock(&(*(this as *const Attributes)).map).insert(CStr::from_ptr(id).to_bytes().to_vec(), v);
    OK
}

/// Hands the attribute stored under `id` to `f`, which copies it out and says whether it fit.
unsafe fn attr_get(this: *mut c_void, id: *const c_char, f: impl FnOnce(&Attr) -> bool) -> TResult {
    if id.is_null() {
        return INVALID_ARG;
    }
    match lock(&(*(this as *const Attributes)).map).get(CStr::from_ptr(id).to_bytes()) {
        Some(a) if f(a) => OK,
        _ => FALSE,
    }
}

unsafe extern "system" fn attr_set_int(this: *mut c_void, id: *const c_char, v: i64) -> TResult {
    attr_set(this, id, Attr::Int(v))
}

unsafe extern "system" fn attr_get_int(this: *mut c_void, id: *const c_char, out: *mut i64) -> TResult {
    attr_get(this, id, |a| match a {
        Attr::Int(v) if !out.is_null() => {
            unsafe { *out = *v };
            true
        }
        _ => false,
    })
}

unsafe extern "system" fn attr_set_float(this: *mut c_void, id: *const c_char, v: f64) -> TResult {
    attr_set(this, id, Attr::Float(v))
}

unsafe extern "system" fn attr_get_float(this: *mut c_void, id: *const c_char, out: *mut f64) -> TResult {
    attr_get(this, id, |a| match a {
        Attr::Float(v) if !out.is_null() => {
            unsafe { *out = *v };
            true
        }
        _ => false,
    })
}

unsafe extern "system" fn attr_set_string(this: *mut c_void, id: *const c_char, s: *const u16) -> TResult {
    if s.is_null() {
        return INVALID_ARG;
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    attr_set(this, id, Attr::Str(std::slice::from_raw_parts(s, n).to_vec()))
}

unsafe extern "system" fn attr_get_string(this: *mut c_void, id: *const c_char, out: *mut u16, size_bytes: u32) -> TResult {
    attr_get(this, id, |a| match a {
        Attr::Str(s) if !out.is_null() && size_bytes >= 2 => {
            let n = s.len().min(size_bytes as usize / 2 - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(s.as_ptr(), out, n);
                *out.add(n) = 0;
            }
            true
        }
        _ => false,
    })
}

unsafe extern "system" fn attr_set_binary(this: *mut c_void, id: *const c_char, data: *const c_void, size: u32) -> TResult {
    let bytes = if data.is_null() || size == 0 { Vec::new() } else { std::slice::from_raw_parts(data as *const u8, size as usize).to_vec() };
    attr_set(this, id, Attr::Bin(bytes))
}

/// The data stays owned by the list and valid until that attribute is replaced.
unsafe extern "system" fn attr_get_binary(this: *mut c_void, id: *const c_char, data: *mut *const c_void, size: *mut u32) -> TResult {
    attr_get(this, id, |a| match a {
        Attr::Bin(b) if !data.is_null() && !size.is_null() => {
            unsafe {
                *data = b.as_ptr() as *const c_void;
                *size = b.len() as u32;
            }
            true
        }
        _ => false,
    })
}

// ---- Edit handler ------------------------------------------------------------------------

/// What the plugin's edit handler shares with the host's UI and audio sides.
pub struct Shared {
    /// Parameter changes on their way to the processor (from the plugin's window or the host).
    pub to_audio: Mutex<rtrb::Producer<(u32, f64)>>,
    /// `restartComponent` flags not yet handled.
    pub restart: AtomicI32,
}

#[repr(C)]
struct Handler {
    vtbl: *const ComponentHandlerVtbl,
    refs: AtomicU32,
    shared: Arc<Shared>,
}

com_object!(Handler, handler_qi, handler_add_ref, handler_release, IID_COMPONENT_HANDLER);

static HANDLER_VTBL: ComponentHandlerVtbl = ComponentHandlerVtbl {
    query_interface: handler_qi,
    add_ref: handler_add_ref,
    release: handler_release,
    begin_edit: handler_mark,
    perform_edit: handler_perform,
    end_edit: handler_mark,
    restart_component: handler_restart,
};

pub fn new_handler(shared: Arc<Shared>) -> *mut c_void {
    Box::into_raw(Box::new(Handler { vtbl: &HANDLER_VTBL, refs: AtomicU32::new(1), shared })) as *mut c_void
}

unsafe extern "system" fn handler_mark(_this: *mut c_void, _id: u32) -> TResult {
    OK
}

unsafe extern "system" fn handler_perform(this: *mut c_void, id: u32, v: f64) -> TResult {
    let h = &*(this as *const Handler);
    let _ = lock(&h.shared.to_audio).push((id, v));
    OK
}

unsafe extern "system" fn handler_restart(this: *mut c_void, flags: i32) -> TResult {
    let h = &*(this as *const Handler);
    h.shared.restart.fetch_or(flags, Relaxed);
    OK
}

// ---- Editor frame ------------------------------------------------------------------------

#[repr(C)]
pub struct Frame {
    vtbl: *const PlugFrameVtbl,
    refs: AtomicU32,
    /// The editor window, or 0 while closed.
    pub hwnd: AtomicIsize,
    /// Set while the host resizes the window for the plugin, so the window's own resize
    /// notification isn't sent back to it.
    pub resizing: AtomicBool,
}

com_object!(Frame, frame_qi, frame_add_ref, frame_release, IID_PLUG_FRAME);

static FRAME_VTBL: PlugFrameVtbl = PlugFrameVtbl { query_interface: frame_qi, add_ref: frame_add_ref, release: frame_release, resize_view: frame_resize };

pub fn new_frame() -> *mut Frame {
    let f = Frame { vtbl: &FRAME_VTBL, refs: AtomicU32::new(1), hwnd: AtomicIsize::new(0), resizing: AtomicBool::new(false) };
    Box::into_raw(Box::new(f))
}

unsafe extern "system" fn frame_resize(this: *mut c_void, view: *mut c_void, r: *mut ViewRect) -> TResult {
    if view.is_null() || r.is_null() {
        return INVALID_ARG;
    }
    let f = &*(this as *const Frame);
    let mut rect = *r;
    let hwnd = f.hwnd.load(Relaxed);
    if hwnd != 0 {
        f.resizing.store(true, Relaxed);
        resize_client(hwnd, rect.width(), rect.height());
        f.resizing.store(false, Relaxed);
    }
    // Some plugins only update their layout from onSize, so the host always follows up with it.
    ((**(view as *const *const PlugViewVtbl)).on_size)(view, &mut rect);
    OK
}

// ---- Memory stream -----------------------------------------------------------------------

#[repr(C)]
struct Stream {
    vtbl: *const BStreamVtbl,
    refs: AtomicU32,
    buf: Mutex<(Vec<u8>, usize)>,
}

com_object!(Stream, stream_qi, stream_add_ref, stream_release, IID_BSTREAM);

static STREAM_VTBL: BStreamVtbl = BStreamVtbl {
    query_interface: stream_qi,
    add_ref: stream_add_ref,
    release: stream_release,
    read: stream_read,
    write: stream_write,
    seek: stream_seek,
    tell: stream_tell,
};

/// A growable in-memory `IBStream` for plugin state.
pub struct MemStream(*mut Stream);

impl MemStream {
    pub fn new(data: Vec<u8>) -> MemStream {
        let s = Stream { vtbl: &STREAM_VTBL, refs: AtomicU32::new(1), buf: Mutex::new((data, 0)) };
        MemStream(Box::into_raw(Box::new(s)))
    }
    pub fn ptr(&self) -> *mut c_void {
        self.0 as *mut c_void
    }
    pub fn rewind(&self) {
        unsafe { lock(&(*self.0).buf).1 = 0 };
    }
    pub fn take(&self) -> Vec<u8> {
        unsafe { std::mem::take(&mut lock(&(*self.0).buf).0) }
    }
}

impl Drop for MemStream {
    fn drop(&mut self) {
        unsafe { stream_release(self.0 as *mut c_void) };
    }
}

unsafe extern "system" fn stream_read(this: *mut c_void, buf: *mut c_void, n: i32, done: *mut i32) -> TResult {
    if buf.is_null() {
        return INVALID_ARG;
    }
    let mut g = lock(&(*(this as *const Stream)).buf);
    let (data, pos) = &mut *g;
    let k = (n.max(0) as usize).min(data.len().saturating_sub(*pos));
    std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buf as *mut u8, k);
    *pos += k;
    if !done.is_null() {
        *done = k as i32;
    }
    OK
}

unsafe extern "system" fn stream_write(this: *mut c_void, buf: *mut c_void, n: i32, done: *mut i32) -> TResult {
    if buf.is_null() || n < 0 {
        return INVALID_ARG;
    }
    let mut g = lock(&(*(this as *const Stream)).buf);
    let (data, pos) = &mut *g;
    let n = n as usize;
    if data.len() < *pos + n {
        data.resize(*pos + n, 0);
    }
    std::ptr::copy_nonoverlapping(buf as *const u8, data.as_mut_ptr().add(*pos), n);
    *pos += n;
    if !done.is_null() {
        *done = n as i32;
    }
    OK
}

unsafe extern "system" fn stream_seek(this: *mut c_void, to: i64, mode: i32, result: *mut i64) -> TResult {
    let mut g = lock(&(*(this as *const Stream)).buf);
    let (data, pos) = &mut *g;
    let base = match mode {
        0 => 0,
        1 => *pos as i64,
        2 => data.len() as i64,
        _ => return INVALID_ARG,
    };
    *pos = (base + to).max(0) as usize;
    if !result.is_null() {
        *result = *pos as i64;
    }
    OK
}

unsafe extern "system" fn stream_tell(this: *mut c_void, out: *mut i64) -> TResult {
    if out.is_null() {
        return INVALID_ARG;
    }
    *out = lock(&(*(this as *const Stream)).buf).1 as i64;
    OK
}

// ---- Real-time parameter and event lists (allocated once, reused every block) --------------

const MAX_POINTS: usize = 16;

#[repr(C)]
pub struct ParamQueue {
    vtbl: *const ParamValueQueueVtbl,
    id: u32,
    count: i32,
    points: [(i32, f64); MAX_POINTS],
}

impl ParamQueue {
    fn push(&mut self, offset: i32, value: f64) -> i32 {
        let n = self.count as usize;
        if n > 0 && self.points[n - 1].0 >= offset {
            // Same moment (or out of order): the newer value wins and the order holds.
            self.points[n - 1].1 = value;
            return n as i32 - 1;
        }
        if n < MAX_POINTS {
            self.points[n] = (offset, value);
            self.count += 1;
            return n as i32;
        }
        self.points[n - 1] = (offset, value);
        n as i32 - 1
    }
}

#[repr(C)]
pub struct ParamChanges {
    vtbl: *const ParameterChangesVtbl,
    count: i32,
    queues: Box<[ParamQueue]>,
}

static CHANGES_VTBL: ParameterChangesVtbl = ParameterChangesVtbl {
    query_interface: changes_qi,
    add_ref: no_ref,
    release: no_ref,
    get_parameter_count: changes_count,
    get_parameter_data: changes_data,
    add_parameter_data: changes_add,
};

static QUEUE_VTBL: ParamValueQueueVtbl = ParamValueQueueVtbl {
    query_interface: queue_qi,
    add_ref: no_ref,
    release: no_ref,
    get_parameter_id: queue_id,
    get_point_count: queue_count,
    get_point: queue_point,
    add_point: queue_add,
};

impl ParamChanges {
    pub fn new(cap: usize) -> Box<ParamChanges> {
        let queues = (0..cap).map(|_| ParamQueue { vtbl: &QUEUE_VTBL, id: 0, count: 0, points: [(0, 0.0); MAX_POINTS] }).collect();
        Box::new(ParamChanges { vtbl: &CHANGES_VTBL, count: 0, queues })
    }
    pub fn ptr(&mut self) -> *mut c_void {
        self as *mut ParamChanges as *mut c_void
    }
    pub fn clear(&mut self) {
        self.count = 0;
    }
    fn queue(&mut self, id: u32) -> Option<(usize, &mut ParamQueue)> {
        let n = self.count as usize;
        if let Some(i) = self.queues[..n].iter().position(|q| q.id == id) {
            return Some((i, &mut self.queues[i]));
        }
        if n == self.queues.len() {
            return None;
        }
        self.count += 1;
        let q = &mut self.queues[n];
        q.id = id;
        q.count = 0;
        Some((n, q))
    }
    /// Points for one parameter must arrive in time order.
    pub fn add(&mut self, id: u32, offset: i32, value: f64) {
        if let Some((_, q)) = self.queue(id) {
            q.push(offset, value);
        }
    }
    /// The final value of every parameter that changed.
    pub fn last_values(&self) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.queues[..self.count as usize].iter().filter(|q| q.count > 0).map(|q| (q.id, q.points[q.count as usize - 1].1))
    }
}

unsafe extern "system" fn changes_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    self_qi(this, iid, obj, &IID_PARAMETER_CHANGES)
}

unsafe extern "system" fn changes_count(this: *mut c_void) -> i32 {
    (*(this as *const ParamChanges)).count
}

unsafe extern "system" fn changes_data(this: *mut c_void, index: i32) -> *mut c_void {
    let c = &mut *(this as *mut ParamChanges);
    if index < 0 || index >= c.count {
        return null_mut();
    }
    &mut c.queues[index as usize] as *mut ParamQueue as *mut c_void
}

unsafe extern "system" fn changes_add(this: *mut c_void, id: *const u32, index: *mut i32) -> *mut c_void {
    if id.is_null() {
        return null_mut();
    }
    match (*(this as *mut ParamChanges)).queue(*id) {
        Some((i, q)) => {
            if !index.is_null() {
                *index = i as i32;
            }
            q as *mut ParamQueue as *mut c_void
        }
        None => null_mut(),
    }
}

unsafe extern "system" fn queue_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    self_qi(this, iid, obj, &IID_PARAM_VALUE_QUEUE)
}

unsafe extern "system" fn queue_id(this: *mut c_void) -> u32 {
    (*(this as *const ParamQueue)).id
}

unsafe extern "system" fn queue_count(this: *mut c_void) -> i32 {
    (*(this as *const ParamQueue)).count
}

unsafe extern "system" fn queue_point(this: *mut c_void, i: i32, offset: *mut i32, value: *mut f64) -> TResult {
    let q = &*(this as *const ParamQueue);
    if i < 0 || i >= q.count {
        return INVALID_ARG;
    }
    let (o, v) = q.points[i as usize];
    if !offset.is_null() {
        *offset = o;
    }
    if !value.is_null() {
        *value = v;
    }
    OK
}

unsafe extern "system" fn queue_add(this: *mut c_void, offset: i32, value: f64, index: *mut i32) -> TResult {
    let i = (*(this as *mut ParamQueue)).push(offset, value);
    if !index.is_null() {
        *index = i;
    }
    OK
}

#[repr(C)]
pub struct EventList {
    vtbl: *const EventListVtbl,
    events: Vec<Event>,
}

static EVENTS_VTBL: EventListVtbl = EventListVtbl {
    query_interface: events_qi,
    add_ref: no_ref,
    release: no_ref,
    get_event_count: events_count,
    get_event: events_get,
    add_event: events_add,
};

impl EventList {
    pub fn new(cap: usize) -> Box<EventList> {
        Box::new(EventList { vtbl: &EVENTS_VTBL, events: Vec::with_capacity(cap) })
    }
    pub fn ptr(&mut self) -> *mut c_void {
        self as *mut EventList as *mut c_void
    }
    pub fn clear(&mut self) {
        self.events.clear();
    }
    /// Never allocates: events past the capacity are dropped.
    pub fn push(&mut self, e: Event) {
        if self.events.len() < self.events.capacity() {
            self.events.push(e);
        }
    }
}

unsafe extern "system" fn events_qi(this: *mut c_void, iid: *const Tuid, obj: *mut *mut c_void) -> TResult {
    self_qi(this, iid, obj, &IID_EVENT_LIST)
}

unsafe extern "system" fn events_count(this: *mut c_void) -> i32 {
    (*(this as *const EventList)).events.len() as i32
}

unsafe extern "system" fn events_get(this: *mut c_void, i: i32, out: *mut Event) -> TResult {
    let l = &*(this as *const EventList);
    match usize::try_from(i).ok().and_then(|i| l.events.get(i)) {
        Some(e) if !out.is_null() => {
            *out = *e;
            OK
        }
        _ => INVALID_ARG,
    }
}

unsafe extern "system" fn events_add(this: *mut c_void, e: *mut Event) -> TResult {
    if e.is_null() {
        return INVALID_ARG;
    }
    let l = &mut *(this as *mut EventList);
    if l.events.len() < l.events.capacity() {
        l.events.push(*e);
        OK
    } else {
        FALSE
    }
}
