extern crate amy;
#[macro_use]
extern crate bitfield;
#[macro_use]
extern crate cstr_macro;
#[macro_use]
extern crate janus_plugin;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_json;

use std::collections::HashMap;
use std::os::raw::{c_int, c_char, c_void};
use std::net::UdpSocket;
use std::sync::{Mutex, RwLock, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use amy::{Event, Poller};
use janus_plugin::{PluginCallbacks, PluginSession, RawPluginResult, PluginResult,
                   PluginResultType, RawJanssonValue, JanssonValue};

#[derive(Debug)]
struct Session {
    handle: *mut PluginSession,
}
unsafe impl std::marker::Send for Session {}
unsafe impl std::marker::Sync for Session {}

#[derive(Debug)]
struct Message {
    handle: *mut PluginSession,
    transaction: *mut c_char,
    message: Option<JanssonValue>,
    jsep: Option<JanssonValue>,
}
unsafe impl std::marker::Send for Message {}

bitfield!{
    struct RtpHeader(MSB0 [u8]);
    impl Debug;
    u8;
    get_version, _: 1, 0;
    get_padding, _: 2, 2;
    get_extension, _: 3, 3;
    get_csrc, _: 7, 4;
    get_marker, set_marker: 8, 8;
    get_payload_type, set_payload_type: 15, 9;
}

lazy_static! {
    static ref CHANNEL: Mutex<Option<mpsc::Sender<Message>>> = Mutex::new(None);
    static ref SESSIONS: RwLock<Vec<Box<Session>>> = RwLock::new(Vec::new());
    static ref SOCKETS: RwLock<HashMap<usize, UdpSocket>> = RwLock::new(HashMap::new());
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

    let poller = Poller::new().unwrap();
    let registrar = poller.get_registrar().unwrap();

    let video_socket = UdpSocket::bind("0.0.0.0:5004").expect("couldn't bind to video socket");
    video_socket.set_nonblocking(true).expect("set_nonblocking call failed");

    let video_socket_id = registrar.register(&video_socket, Event::Read).unwrap();
    SOCKETS.write().unwrap().insert(video_socket_id, video_socket);

    // let audio_socket = UdpSocket::bind("0.0.0.0:5002").expect("couldn't bind to audio socket");
    // audio_socket.set_nonblocking(true).expect("set_nonblocking call failed");

    // let audio_socket_id = registrar.register(&audio_socket, Event::Read).unwrap();
    // SOCKETS.write().unwrap().insert(audio_socket_id, audio_socket);

    thread::spawn(move || { poll(poller) });
    thread::spawn(move || { message_handler(rx); });

    0
}

extern "C" fn destroy() {}

extern "C" fn create_session(handle: *mut PluginSession, _error: *mut c_int) {
    let mut session = Box::new(Session { handle: handle });

    let handle = unsafe { &mut *handle };
    handle.plugin_handle = session.as_mut() as *mut Session as *mut c_void;

    println!("--> create_session: {:?}", session);

    SESSIONS.write().unwrap().push(session);
}

extern "C" fn query_session(_handle: *mut PluginSession) -> *mut RawJanssonValue {
    std::ptr::null_mut()
}

extern "C" fn destroy_session(_handle: *mut PluginSession, _error: *mut c_int) {}

extern "C" fn handle_message(
    handle: *mut PluginSession,
    transaction: *mut c_char,
    raw_message: *mut RawJanssonValue,
    raw_jsep: *mut RawJanssonValue,
) -> *mut RawPluginResult {

    janus_plugin::log(
        janus_plugin::LogLevel::Verb,
        "--> janus_streaming_handle_message!!!",
    );

    let message = unsafe { JanssonValue::new(raw_message) }.unwrap();
    let message_json: serde_json::Value = parse_message(message.clone());
    println!("{:?}", message_json);

    if message_json["request"] == json!("watch") || message_json["request"] == json!("start") {
        let jsep = unsafe { JanssonValue::new(raw_jsep) };

        let msg = Message {
            handle: handle,
            transaction: transaction,
            message: Some(message),
            jsep: jsep,
        };

        let mutex = CHANNEL.lock().unwrap();
        let tx = mutex.as_ref().unwrap();

        janus_plugin::log(
            janus_plugin::LogLevel::Verb,
            "--> Sending message to channel",
        );
        tx.send(msg).expect(
            "Sending to channel has failed",
        );

        PluginResult::new(
            PluginResultType::JANUS_PLUGIN_OK_WAIT,
            // std::ptr::null_mut(),
            cstr!("REzzz"),
            None,
        ).into_raw()
    } else {
        unreachable!()
    }
}

extern "C" fn setup_media(_handle: *mut PluginSession) {
    janus_plugin::log(
        janus_plugin::LogLevel::Verb,
        "--> setup_media",
    );
}

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

fn message_handler(rx: mpsc::Receiver<Message>) {
    janus_plugin::log(janus_plugin::LogLevel::Verb, "--> Start handling thread");

    let watch_request = json!("watch");

    for received in rx.iter() {
        janus_plugin::log(
            janus_plugin::LogLevel::Verb,
            &format!("--> message_handler, received: {:?}", received),
        );

        let message = received.message.unwrap();
        let message_json: serde_json::Value = parse_message(message);

        if message_json["request"] == watch_request {
            janus_plugin::log(janus_plugin::LogLevel::Verb, "--> Handling watch request");

            let jsep_json = json!({ "type": "offer", "sdp": generate_sdp_offer() });
            let jsep: JanssonValue = JanssonValue::from_str(
                &jsep_json.to_string(),
                janus_plugin::JanssonDecodingFlags::empty(),
            ).unwrap();

            let event_json = json!({ "result": "ok" });
            let event: JanssonValue = JanssonValue::from_str(
                &event_json.to_string(),
                janus_plugin::JanssonDecodingFlags::empty(),
            ).unwrap();

            let push_event_fn = acquire_gateway().push_event;
            janus_plugin::get_result(push_event_fn(
                received.handle,
                &mut PLUGIN,
                received.transaction,
                event.as_mut_ref(),
                jsep.as_mut_ref(),
            )).expect("Pushing event has failed");
        }
    }
}

fn parse_message(msg: JanssonValue) -> serde_json::Value {
    let message_str: String = msg.to_string(janus_plugin::JanssonEncodingFlags::empty());
    serde_json::from_str(&message_str).unwrap()
}

fn generate_sdp_offer() -> String {
    let mut sdp = String::new();

    sdp.push_str("v=0\r\n");

    let sys_time = SystemTime::now();
    let time_since = sys_time.duration_since(UNIX_EPOCH).unwrap();
    let nanos = time_since.as_secs() * 1_000_000 + time_since.subsec_nanos() as u64;
    sdp.push_str(&format!("o=- {} {} IN IP4 127.0.0.1\r\n", nanos, nanos));

    sdp.push_str("s=Mountpoint 1\r\n");
    sdp.push_str("t=0 0\r\n");

    sdp.push_str("m=audio 1 RTP/SAVPF 111\r\n");
    sdp.push_str("c=IN IP4 1.1.1.1\r\n");
    sdp.push_str("a=rtpmap:111 opus/48000/2\r\n");
    sdp.push_str("a=sendonly\r\n");

    sdp.push_str("m=video 1 RTP/SAVPF 100\r\n");
    sdp.push_str("c=IN IP4 1.1.1.1\r\n");
    sdp.push_str("a=rtpmap:100 VP8/90000\r\n");
    sdp.push_str("a=rtcp-fb:100 nack\r\n");
    sdp.push_str("a=rtcp-fb:100 goog-remb\r\n");
    sdp.push_str("a=sendonly\r\n");

    janus_plugin::log(
        janus_plugin::LogLevel::Verb,
        &format!("--> Going to offer this SDP: {:?}", sdp),
    );

    sdp
}

fn poll(mut poller: Poller) {
    janus_plugin::log(janus_plugin::LogLevel::Verb, "--> Start polling thread");

    let mut buf: [u8; 1500] = [0; 1500];
    let sockets = SOCKETS.read().unwrap();
    println!("{:?}", sockets);
    let relay_rtp_fn = acquire_gateway().relay_rtp;

    loop {
        let notifications = poller.wait(0).unwrap();
        for n in notifications {
            // println!("{:?}", n);
            if let Some(socket) = sockets.get(&n.id) {
                match n.event {
                    Event::Read => {
                        let (number_of_bytes, src_addr) = socket.recv_from(&mut buf).expect("Didn't receive data");
                        // println!("number_of_bytes: {:?}, src_addr: {:?}", number_of_bytes, src_addr);

                        let ptype = if n.id == 1 { 100 } else { 111 };

                        // Works!!!
                        let mut header = RtpHeader(&mut buf[..]);
                        header.set_payload_type(ptype);

                        // // println!("--> {:?}", boxed_session);
                        let boxed_session = &SESSIONS.read().unwrap()[0];

                        let is_video = if n.id == 1 { 1 } else { 0 };
                        // let ptr = (&buf).as_ptr();
                        // let buf_ptr = ptr as *mut u8 as *mut i8;

                        // unsafe {
                        //     janus_rtp_set_type(buf_ptr, ptype as i32);
                        // }
                        relay_rtp_fn(
                            boxed_session.handle,
                            is_video as c_int,
                            // &buf[0] as *const u8 as *mut c_char,
                            // buf[0..number_of_bytes].as_mut_ptr() as *mut c_char,
                            // ptr as *mut u8 as *mut i8,
                            // buf_ptr,
                            header.0.as_mut_ptr() as *mut c_char,
                            number_of_bytes as c_int,
                        );
                    },
                    _ => ()
                }
            }
        }
    }
}

fn acquire_gateway() -> &'static PluginCallbacks {
    unsafe { GATEWAY }.expect("Gateway is NONE")
}

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

extern "C" {
    fn janus_rtp_set_type(buf: *mut c_char, ptype: c_int);
}
