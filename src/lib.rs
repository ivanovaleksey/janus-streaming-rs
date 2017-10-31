#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate janus_plugin;
#[macro_use]
extern crate lazy_static;

use std::os::raw::{c_int, c_char, c_void};
use std::sync::{Mutex, RwLock, mpsc};
use janus_plugin::{PluginCallbacks, PluginSession, RawPluginResult, PluginResult,
                   PluginResultType, RawJanssonValue, JanssonValue};

#[derive(Debug)]
struct Session;

#[derive(Debug)]
struct Message {
    handle: *mut PluginSession,
    transaction: *mut c_char,
    message: Option<JanssonValue>,
    jsep: Option<JanssonValue>,
}
unsafe impl std::marker::Send for Message {}

lazy_static! {
    static ref CHANNEL: Mutex<Option<mpsc::Sender<Message>>> = Mutex::new(None);
    static ref SESSIONS: RwLock<Vec<Box<Session>>> = RwLock::new(Vec::new());
}

static mut GATEWAY: Option<&PluginCallbacks> = None;

const METADATA: janus_plugin::PluginMetadata = janus_plugin::PluginMetadata {
    version: 1,
    version_str: cstr!("0.1"),
    description: cstr!("Streaming plugin"),
    name: cstr!("Streaming plugin"),
    author: cstr!("Aleksey Ivanov"),
    package: cstr!("janus.plugin.streaming"),
};

extern "C" fn init(callback: *mut PluginCallbacks, _config_path: *const c_char) -> c_int {
    unsafe {
        let callback = callback.as_ref().unwrap();
        GATEWAY = Some(callback);
    }

    let (tx, rx) = mpsc::channel();
    *(CHANNEL.lock().unwrap()) = Some(tx);

    std::thread::spawn(move || { message_handler(rx); });
    0
}

extern "C" fn destroy() {}

extern "C" fn create_session(handle: *mut PluginSession, _error: *mut c_int) {
    let handle = unsafe { &mut *handle };
    let mut session = Box::new(Session {});

    handle.plugin_handle = session.as_mut() as *mut Session as *mut c_void;
    SESSIONS.write().unwrap().push(session);
}

extern "C" fn query_session(_handle: *mut PluginSession) -> *mut RawJanssonValue {
    std::ptr::null_mut()
}

extern "C" fn destroy_session(_handle: *mut PluginSession, _error: *mut c_int) {}

extern "C" fn handle_message(
    handle: *mut PluginSession,
    transaction: *mut c_char,
    message: *mut RawJanssonValue,
    jsep: *mut RawJanssonValue,
) -> *mut RawPluginResult {

    janus_plugin::log(
        janus_plugin::LogLevel::Verb,
        "--> janus_streaming_handle_message!!!",
    );

    let _session: &Session = unsafe {
        let handle_ref = handle.as_ref().unwrap();
        (handle_ref.plugin_handle as *mut Session).as_ref()
    }.unwrap();

    let mutex = CHANNEL.lock().unwrap();
    let tx = mutex.as_ref().unwrap();

    let message = unsafe { JanssonValue::new(message) };
    let jsep = unsafe { JanssonValue::new(jsep) };

    let msg = Message {
        handle: handle,
        transaction: transaction,
        message: message,
        jsep: jsep,
    };
    janus_plugin::log(
        janus_plugin::LogLevel::Verb,
        "--> sending message to channel",
    );
    tx.send(msg).expect(
        "Sending to channel has failed",
    );

    let result = PluginResult::new(
        PluginResultType::JANUS_PLUGIN_OK_WAIT,
        cstr!("Rust string"),
        None,
    );
    result.into_raw()
}

extern "C" fn setup_media(_handle: *mut PluginSession) {}

extern "C" fn hangup_media(_handle: *mut PluginSession) {}

extern "C" fn incoming_rtp(
    _handle: *mut PluginSession,
    _video: c_int,
    _buf: *mut c_char,
    _len: c_int,
) {
}

extern "C" fn incoming_rtcp(
    _handle: *mut PluginSession,
    _video: c_int,
    _buf: *mut c_char,
    _len: c_int,
) {
}

extern "C" fn incoming_data(_handle: *mut PluginSession, _buf: *mut c_char, _len: c_int) {}

extern "C" fn slow_link(_handle: *mut PluginSession, _uplink: c_int, _video: c_int) {}

fn message_handler(_rx: mpsc::Receiver<Message>) {}

const PLUGIN: janus_plugin::Plugin = build_plugin!(
    METADATA,
    init,
    destroy,
    create_session,
    handle_message,
    setup_media,
    incoming_rtp,
    incoming_rtcp,
    incoming_data,
    slow_link,
    hangup_media,
    destroy_session,
    query_session
);

export_plugin!(&PLUGIN);
