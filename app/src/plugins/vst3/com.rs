//! The VST3 binary interface as a host on 64-bit Windows sees it: interface ids, the plain
//! structures passed across it, vtable layouts and a reference-counted interface pointer.
//! Written from the published interface definitions; every layout is checked at compile time.
#![allow(dead_code)]

use std::ffi::{c_char, c_void};
use std::marker::PhantomData;
use std::mem::{offset_of, size_of};
use std::ptr::null_mut;

pub type TResult = i32;
pub const OK: TResult = 0;
pub const FALSE: TResult = 1;
pub const NO_INTERFACE: TResult = 0x8000_4002u32 as i32;
pub const NOT_IMPLEMENTED: TResult = 0x8000_4001u32 as i32;
pub const INVALID_ARG: TResult = 0x8007_0057u32 as i32;

pub type Tuid = [u8; 16];

/// Interface ids in the COM-compatible byte order used on Windows.
pub const fn uid(l1: u32, l2: u32, l3: u32, l4: u32) -> Tuid {
    [
        l1 as u8,
        (l1 >> 8) as u8,
        (l1 >> 16) as u8,
        (l1 >> 24) as u8,
        (l2 >> 16) as u8,
        (l2 >> 24) as u8,
        l2 as u8,
        (l2 >> 8) as u8,
        (l3 >> 24) as u8,
        (l3 >> 16) as u8,
        (l3 >> 8) as u8,
        l3 as u8,
        (l4 >> 24) as u8,
        (l4 >> 16) as u8,
        (l4 >> 8) as u8,
        l4 as u8,
    ]
}

pub const IID_FUNKNOWN: Tuid = uid(0, 0, 0xC000_0000, 0x0000_0046);
pub const IID_PLUGIN_BASE: Tuid = uid(0x22888DDB, 0x156E45AE, 0x8358B348, 0x08190625);
pub const IID_PLUGIN_FACTORY: Tuid = uid(0x7A4D811C, 0x52114A1F, 0xAED9D2EE, 0x0B43BF9F);
pub const IID_PLUGIN_FACTORY2: Tuid = uid(0x0007B650, 0xF24B4C0B, 0xA464EDB9, 0xF00B2ABB);
pub const IID_PLUGIN_FACTORY3: Tuid = uid(0x4555A2AB, 0xC1234E57, 0x9B122910, 0x36878931);
pub const IID_COMPONENT: Tuid = uid(0xE831FF31, 0xF2D54301, 0x928EBBEE, 0x25697802);
pub const IID_AUDIO_PROCESSOR: Tuid = uid(0x42043F99, 0xB7DA453C, 0xA569E79D, 0x9AAEC33D);
pub const IID_EDIT_CONTROLLER: Tuid = uid(0xDCD7BBE3, 0x7742448D, 0xA874AACC, 0x979C759E);
pub const IID_MIDI_MAPPING: Tuid = uid(0xDF0FF9F7, 0x49B74669, 0xB63AB732, 0x7ADBF5E5);
pub const IID_COMPONENT_HANDLER: Tuid = uid(0x93A0BEA3, 0x0BD045DB, 0x8E890B0C, 0xC1E46AC6);
pub const IID_HOST_APPLICATION: Tuid = uid(0x58E595CC, 0xDB2D4969, 0x8B6AAF8C, 0x36A664E5);
pub const IID_CONNECTION_POINT: Tuid = uid(0x70A4156F, 0x6E6E4026, 0x989148BF, 0xAA60D8D1);
pub const IID_MESSAGE: Tuid = uid(0x936F033B, 0xC6C047DB, 0xBB0882F8, 0x13C1E613);
pub const IID_ATTRIBUTE_LIST: Tuid = uid(0x1E5F0AEB, 0xCC7F4533, 0xA2544011, 0x38AD5EE4);
pub const IID_BSTREAM: Tuid = uid(0xC3BF6EA2, 0x30994752, 0x9B6BF990, 0x1EE33E9B);
pub const IID_PLUG_VIEW: Tuid = uid(0x5BC32507, 0xD06049EA, 0xA6151B52, 0x2B755B29);
pub const IID_PLUG_FRAME: Tuid = uid(0x367FAF01, 0xAFA94693, 0x8D4DA2A0, 0xED0882A3);
pub const IID_PARAMETER_CHANGES: Tuid = uid(0xA4779663, 0x0BB64A56, 0xB44384A8, 0x466FEB9D);
pub const IID_PARAM_VALUE_QUEUE: Tuid = uid(0x01263A18, 0xED074F6F, 0x98C9D356, 0x4686F9BA);
pub const IID_EVENT_LIST: Tuid = uid(0x3A2C4214, 0x346349FE, 0xB2C4F397, 0xB9695A44);

pub const MEDIA_AUDIO: i32 = 0;
pub const MEDIA_EVENT: i32 = 1;
pub const DIR_IN: i32 = 0;
pub const DIR_OUT: i32 = 1;
pub const SAMPLE_32: i32 = 0;
pub const MODE_REALTIME: i32 = 0;

pub const PARAM_READ_ONLY: i32 = 1 << 1;
pub const PARAM_HIDDEN: i32 = 1 << 4;
pub const PARAM_PROGRAM_CHANGE: i32 = 1 << 15;
pub const PARAM_BYPASS: i32 = 1 << 16;

pub const RESTART_PARAM_VALUES: i32 = 1 << 2;
pub const RESTART_LATENCY: i32 = 1 << 3;
pub const RESTART_PARAM_TITLES: i32 = 1 << 4;
pub const RESTART_MIDI_CC: i32 = 1 << 5;

pub const CTX_PLAYING: u32 = 1 << 1;
pub const CTX_PROJECT_MUSIC_VALID: u32 = 1 << 9;
pub const CTX_TEMPO_VALID: u32 = 1 << 10;
pub const CTX_BAR_VALID: u32 = 1 << 11;
pub const CTX_TIME_SIG_VALID: u32 = 1 << 13;
pub const CTX_CONT_TIME_VALID: u32 = 1 << 17;

pub const EVENT_NOTE_ON: u16 = 0;
pub const EVENT_NOTE_OFF: u16 = 1;
pub const EVENT_POLY_PRESSURE: u16 = 3;
pub const EVENT_IS_LIVE: u16 = 1;

/// MIDI controller numbers past the 128 CCs that `IMidiMapping` understands.
pub const CTRL_AFTERTOUCH: usize = 128;
pub const CTRL_PITCH_BEND: usize = 129;

#[repr(C)]
pub struct FactoryInfo {
    pub vendor: [u8; 64],
    pub url: [u8; 256],
    pub email: [u8; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct ClassInfo {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

#[repr(C)]
pub struct ClassInfo2 {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u8; 64],
    pub version: [u8; 64],
    pub sdk_version: [u8; 64],
}

#[repr(C)]
pub struct ClassInfoW {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u16; 64],
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u16; 64],
    pub version: [u16; 64],
    pub sdk_version: [u16; 64],
}

#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: [u16; 128],
    pub bus_type: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct ParameterInfo {
    pub id: u32,
    pub title: [u16; 128],
    pub short_title: [u16; 128],
    pub units: [u16; 128],
    pub step_count: i32,
    pub default_normalized: f64,
    pub unit_id: i32,
    pub flags: i32,
}

#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub sample_size: i32,
    pub max_samples: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    pub buffers: *mut *mut f32,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub in_params: *mut c_void,
    pub out_params: *mut c_void,
    pub in_events: *mut c_void,
    pub out_events: *mut c_void,
    pub context: *mut ProcessContext,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProcessContext {
    pub state: u32,
    pub sample_rate: f64,
    pub project_time_samples: i64,
    pub system_time: i64,
    pub continuous_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub cycle_start_music: f64,
    pub cycle_end_music: f64,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub chord: [u8; 4],
    pub smpte_offset_subframes: i32,
    pub frame_rate: [u32; 2],
    pub samples_to_next_clock: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ViewRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ViewRect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOn {
    pub channel: i16,
    pub pitch: i16,
    pub tuning: f32,
    pub velocity: f32,
    pub length: i32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOff {
    pub channel: i16,
    pub pitch: i16,
    pub velocity: f32,
    pub note_id: i32,
    pub tuning: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PolyPressure {
    pub channel: i16,
    pub pitch: i16,
    pub pressure: f32,
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union EventBody {
    pub note_on: NoteOn,
    pub note_off: NoteOff,
    pub poly: PolyPressure,
    pub raw: [u64; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub bus_index: i32,
    pub sample_offset: i32,
    pub ppq_position: f64,
    pub flags: u16,
    pub kind: u16,
    pub body: EventBody,
}

const _: () = {
    assert!(size_of::<FactoryInfo>() == 452);
    assert!(size_of::<ClassInfo>() == 116);
    assert!(size_of::<ClassInfo2>() == 440);
    assert!(size_of::<ClassInfoW>() == 696);
    assert!(size_of::<BusInfo>() == 276);
    assert!(size_of::<ParameterInfo>() == 792);
    assert!(offset_of!(ParameterInfo, default_normalized) == 776);
    assert!(size_of::<ProcessSetup>() == 24);
    assert!(size_of::<AudioBusBuffers>() == 24);
    assert!(size_of::<ProcessData>() == 80);
    assert!(offset_of!(ProcessData, inputs) == 24);
    assert!(size_of::<ProcessContext>() == 112);
    assert!(offset_of!(ProcessContext, tempo) == 72);
    assert!(offset_of!(ProcessContext, samples_to_next_clock) == 104);
    assert!(size_of::<Event>() == 48);
    assert!(offset_of!(Event, body) == 24);
};

/// Declares a vtable: the three `FUnknown` slots followed by the interface's own methods.
macro_rules! vtbl {
    ($name:ident { $($f:ident($($a:ty),*) -> $r:ty;)* }) => {
        #[repr(C)]
        pub struct $name {
            pub query_interface: unsafe extern "system" fn(*mut c_void, *const Tuid, *mut *mut c_void) -> TResult,
            pub add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
            pub release: unsafe extern "system" fn(*mut c_void) -> u32,
            $(pub $f: unsafe extern "system" fn(*mut c_void $(, $a)*) -> $r,)*
        }
    };
}

vtbl!(UnknownVtbl {});

vtbl!(FactoryVtbl {
    get_factory_info(*mut FactoryInfo) -> TResult;
    count_classes() -> i32;
    get_class_info(i32, *mut ClassInfo) -> TResult;
    create_instance(*const Tuid, *const Tuid, *mut *mut c_void) -> TResult;
});

vtbl!(Factory2Vtbl {
    get_factory_info(*mut FactoryInfo) -> TResult;
    count_classes() -> i32;
    get_class_info(i32, *mut ClassInfo) -> TResult;
    create_instance(*const Tuid, *const Tuid, *mut *mut c_void) -> TResult;
    get_class_info2(i32, *mut ClassInfo2) -> TResult;
});

vtbl!(Factory3Vtbl {
    get_factory_info(*mut FactoryInfo) -> TResult;
    count_classes() -> i32;
    get_class_info(i32, *mut ClassInfo) -> TResult;
    create_instance(*const Tuid, *const Tuid, *mut *mut c_void) -> TResult;
    get_class_info2(i32, *mut ClassInfo2) -> TResult;
    get_class_info_unicode(i32, *mut ClassInfoW) -> TResult;
    set_host_context(*mut c_void) -> TResult;
});

vtbl!(ComponentVtbl {
    initialize(*mut c_void) -> TResult;
    terminate() -> TResult;
    get_controller_class_id(*mut Tuid) -> TResult;
    set_io_mode(i32) -> TResult;
    get_bus_count(i32, i32) -> i32;
    get_bus_info(i32, i32, i32, *mut BusInfo) -> TResult;
    get_routing_info(*mut c_void, *mut c_void) -> TResult;
    activate_bus(i32, i32, i32, u8) -> TResult;
    set_active(u8) -> TResult;
    set_state(*mut c_void) -> TResult;
    get_state(*mut c_void) -> TResult;
});

vtbl!(AudioProcessorVtbl {
    set_bus_arrangements(*mut u64, i32, *mut u64, i32) -> TResult;
    get_bus_arrangement(i32, i32, *mut u64) -> TResult;
    can_process_sample_size(i32) -> TResult;
    get_latency_samples() -> u32;
    setup_processing(*mut ProcessSetup) -> TResult;
    set_processing(u8) -> TResult;
    process(*mut ProcessData) -> TResult;
    get_tail_samples() -> u32;
});

vtbl!(EditControllerVtbl {
    initialize(*mut c_void) -> TResult;
    terminate() -> TResult;
    set_component_state(*mut c_void) -> TResult;
    set_state(*mut c_void) -> TResult;
    get_state(*mut c_void) -> TResult;
    get_parameter_count() -> i32;
    get_parameter_info(i32, *mut ParameterInfo) -> TResult;
    get_param_string_by_value(u32, f64, *mut u16) -> TResult;
    get_param_value_by_string(u32, *const u16, *mut f64) -> TResult;
    normalized_param_to_plain(u32, f64) -> f64;
    plain_param_to_normalized(u32, f64) -> f64;
    get_param_normalized(u32) -> f64;
    set_param_normalized(u32, f64) -> TResult;
    set_component_handler(*mut c_void) -> TResult;
    create_view(*const c_char) -> *mut c_void;
});

vtbl!(MidiMappingVtbl {
    get_midi_controller_assignment(i32, i16, i16, *mut u32) -> TResult;
});

vtbl!(ConnectionPointVtbl {
    connect(*mut c_void) -> TResult;
    disconnect(*mut c_void) -> TResult;
    notify(*mut c_void) -> TResult;
});

vtbl!(PlugViewVtbl {
    is_platform_type_supported(*const c_char) -> TResult;
    attached(*mut c_void, *const c_char) -> TResult;
    removed() -> TResult;
    on_wheel(f32) -> TResult;
    on_key_down(u16, i16, i16) -> TResult;
    on_key_up(u16, i16, i16) -> TResult;
    get_size(*mut ViewRect) -> TResult;
    on_size(*mut ViewRect) -> TResult;
    on_focus(u8) -> TResult;
    set_frame(*mut c_void) -> TResult;
    can_resize() -> TResult;
    check_size_constraint(*mut ViewRect) -> TResult;
});

// Interfaces the host implements.

vtbl!(HostApplicationVtbl {
    get_name(*mut u16) -> TResult;
    create_instance(*const Tuid, *const Tuid, *mut *mut c_void) -> TResult;
});

vtbl!(ComponentHandlerVtbl {
    begin_edit(u32) -> TResult;
    perform_edit(u32, f64) -> TResult;
    end_edit(u32) -> TResult;
    restart_component(i32) -> TResult;
});

vtbl!(PlugFrameVtbl {
    resize_view(*mut c_void, *mut ViewRect) -> TResult;
});

vtbl!(BStreamVtbl {
    read(*mut c_void, i32, *mut i32) -> TResult;
    write(*mut c_void, i32, *mut i32) -> TResult;
    seek(i64, i32, *mut i64) -> TResult;
    tell(*mut i64) -> TResult;
});

vtbl!(MessageVtbl {
    get_message_id() -> *const c_char;
    set_message_id(*const c_char) -> ();
    get_attributes() -> *mut c_void;
});

vtbl!(AttributeListVtbl {
    set_int(*const c_char, i64) -> TResult;
    get_int(*const c_char, *mut i64) -> TResult;
    set_float(*const c_char, f64) -> TResult;
    get_float(*const c_char, *mut f64) -> TResult;
    set_string(*const c_char, *const u16) -> TResult;
    get_string(*const c_char, *mut u16, u32) -> TResult;
    set_binary(*const c_char, *const c_void, u32) -> TResult;
    get_binary(*const c_char, *mut *const c_void, *mut u32) -> TResult;
});

vtbl!(ParameterChangesVtbl {
    get_parameter_count() -> i32;
    get_parameter_data(i32) -> *mut c_void;
    add_parameter_data(*const u32, *mut i32) -> *mut c_void;
});

vtbl!(ParamValueQueueVtbl {
    get_parameter_id() -> u32;
    get_point_count() -> i32;
    get_point(i32, *mut i32, *mut f64) -> TResult;
    add_point(i32, f64, *mut i32) -> TResult;
});

vtbl!(EventListVtbl {
    get_event_count() -> i32;
    get_event(i32, *mut Event) -> TResult;
    add_event(*mut Event) -> TResult;
});

/// An owned reference to a plugin interface; released on drop.
pub struct Com<V> {
    ptr: *mut c_void,
    _v: PhantomData<*const V>,
}

// Plugin objects are free-threaded as far as the VST3 threading rules allow; the host keeps to them.
unsafe impl<V> Send for Com<V> {}
unsafe impl<V> Sync for Com<V> {}

impl<V> Com<V> {
    /// Takes over one reference.
    pub unsafe fn from_raw(ptr: *mut c_void) -> Option<Com<V>> {
        (!ptr.is_null()).then_some(Com { ptr, _v: PhantomData })
    }

    /// Adds a reference to a borrowed pointer.
    pub unsafe fn from_borrowed(ptr: *mut c_void) -> Option<Com<V>> {
        if ptr.is_null() {
            return None;
        }
        add_ref(ptr);
        Some(Com { ptr, _v: PhantomData })
    }

    pub fn ptr(&self) -> *mut c_void {
        self.ptr
    }

    pub fn vt(&self) -> &V {
        unsafe { &**(self.ptr as *const *const V) }
    }

    pub fn query<W>(&self, iid: &Tuid) -> Option<Com<W>> {
        let mut out = null_mut();
        unsafe {
            let unk = &**(self.ptr as *const *const UnknownVtbl);
            if (unk.query_interface)(self.ptr, iid, &mut out) == OK {
                Com::from_raw(out)
            } else {
                None
            }
        }
    }
}

impl<V> Clone for Com<V> {
    fn clone(&self) -> Self {
        unsafe { add_ref(self.ptr) };
        Com { ptr: self.ptr, _v: PhantomData }
    }
}

impl<V> Drop for Com<V> {
    fn drop(&mut self) {
        unsafe { release(self.ptr) };
    }
}

pub unsafe fn add_ref(p: *mut c_void) -> u32 {
    ((**(p as *const *const UnknownVtbl)).add_ref)(p)
}

pub unsafe fn release(p: *mut c_void) -> u32 {
    ((**(p as *const *const UnknownVtbl)).release)(p)
}

pub fn wstr(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// Writes `s` NUL-terminated into a UTF-16 buffer of `cap` units.
pub unsafe fn write_wstr(dst: *mut u16, cap: usize, s: &str) {
    if dst.is_null() || cap == 0 {
        return;
    }
    let mut n = 0;
    for u in s.encode_utf16().take(cap - 1) {
        *dst.add(n) = u;
        n += 1;
    }
    *dst.add(n) = 0;
}

pub fn hex(id: &Tuid) -> String {
    id.iter().map(|b| format!("{b:02X}")).collect()
}

pub fn parse_hex(s: &str) -> Option<Tuid> {
    let s = s.trim();
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut id = [0u8; 16];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(id)
}
