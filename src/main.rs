mod traffic_meter;

use log::{debug, error, info};
use rand::Rng;
use std::io::prelude::*;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

use pulse::sample::{Format, Spec};
use pulse::stream::Direction;
use simple_pulse::Simple;

use p2p_handshake::{
    client::get_peer_addr,
    crypto::{Sealed, SymmetricKey},
    error::{Error, Result},
    message::{recv_from, send_to},
};

use traffic_meter::TrafficMeter;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
enum Message {
    Heartbeat,
    Text(String),
    Opus(Vec<u8>),
}

#[derive(Debug)]
struct Config {
    audio_ch: u8,
    audio_rate: u32,
    buf_size: usize,
}

fn spawn_command_thread(
    sock: Arc<UdpSocket>,
    key: Arc<Mutex<SymmetricKey>>,
    peer_addr: SocketAddr,
    traffic: Arc<TrafficMeter>,
) {
    println!("peer-to-peer voice chat example");
    println!("['?' to show available commands]");
    spawn(move || {
        || -> Result<()> {
            loop {
                print!("command >>> ");
                std::io::stdout().flush().unwrap();

                let mut buffer = String::new();
                std::io::stdin().read_line(&mut buffer)?;

                match buffer.as_str().trim() {
                    "" => {
                        continue;
                    }
                    "?" => {
                        println!("?\t\t: show available commands");
                        println!("stat\t\t: show traffic statistics");
                        println!("volume+\t\t: mic volume up (10%)");
                        println!("volume-\t\t: mic volume down (10%)");
                        println!("mute\t\t: mic mute");
                        println!("unmute\t\t: mic unmute");
                        println!("text <message>\t: send a text message");
                    }
                    "stat" => {
                        println!("{}", traffic);
                    }
                    "volume+" => {
                        error!("not yet implemented");
                    }
                    "volume-" => {
                        error!("not yet implemented");
                    }
                    "mute" => {
                        error!("not yet implemented");
                    }
                    "unmute" => {
                        error!("not yet implemented");
                    }
                    cmd if cmd.starts_with("text ") => {
                        let text = cmd.strip_prefix("text ").unwrap();
                        let mut key = key.lock().unwrap();
                        let enc_msg = key.encrypt(Message::Text(text.into()))?;
                        send_to(enc_msg, &sock, peer_addr)?;
                    }
                    cmd => {
                        error!("unknown command: {}", cmd);
                    }
                }
            }
        }()
        .unwrap_or_else(|e| error!("stdin thread panicked: {}", e))
    });
}

fn spawn_pulseaudio_input_thread(
    sock: Arc<UdpSocket>,
    key: Arc<Mutex<SymmetricKey>>,
    peer_addr: SocketAddr,
    config: Arc<Config>,
    traffic: Arc<TrafficMeter>,
) {
    let spec = Spec {
        format: Format::S16NE,
        channels: config.audio_ch,
        rate: config.audio_rate,
    };
    assert!(spec.is_valid());

    let record = Simple::new(
        None,              // Use the default server
        "p2p-voice-chat",  // Our application’s name
        Direction::Record, // We want a record stream
        None,              // Use the default device
        "recording",       // Description of our stream
        &spec,             // Our sample format
        None,              // Use default channel map
        None,              // Use default buffering attributes
    )
    .unwrap_or_else(|e| panic!("pulseaudio error: {:?}", e.to_string()));

    spawn(move || {
        || -> Result<()> {
            let mut buf = vec![0i16; config.buf_size / 2];
            let buf = {
                let (p, buf, s) = unsafe { buf.align_to_mut::<u8>() };
                assert!(p.is_empty() && s.is_empty());
                assert!(buf.len() >= config.buf_size);
                &mut buf[..config.buf_size]
            };

            let ch = match config.audio_ch {
                1 => opus::Channels::Mono,
                2 => opus::Channels::Stereo,
                _ => panic!("unsupported channel"),
            };
            let mut opus = opus::Encoder::new(spec.rate, ch, opus::Application::Voip).unwrap();
            let mut encoded = vec![0; config.buf_size];

            loop {
                record.read(buf).unwrap();
                let (_, bufi16, _) = unsafe { buf.align_to_mut() };
                let sz = opus.encode(bufi16, &mut encoded).unwrap();
                let encoded = &encoded[..sz];

                let mut key = key.lock().unwrap();
                let enc_msg = key.encrypt(Message::Opus(encoded.to_vec()))?;
                send_to(enc_msg, &sock, peer_addr)?;

                traffic.sent_bytes(encoded.len());
            }
        }()
        .unwrap_or_else(|e| error!("heartbeat thread panicked: {}", e))
    });
}

fn spawn_heartbeat_thread(
    sock: Arc<UdpSocket>,
    key: Arc<Mutex<SymmetricKey>>,
    peer_addr: SocketAddr,
) {
    spawn(move || {
        || -> Result<()> {
            loop {
                {
                    let mut key = key.lock().unwrap();
                    let enc_msg = key.encrypt(Message::Heartbeat)?;
                    send_to(enc_msg, &sock, peer_addr)?;
                }
                sleep(Duration::from_secs(5));
            }
        }()
        .unwrap_or_else(|e| error!("heartbeat thread panicked: {}", e))
    });
}

fn voice_chat(
    sock: UdpSocket,
    my_addr: SocketAddr,
    peer_addr: SocketAddr,
    preshared_key: &[u8],
    config: Config,
) -> Result<()> {
    let sock = Arc::new(sock);

    info!("config = {:?}", config);
    let config = Arc::new(config);

    // `key_id` is needed to agree the same "direction" of encryption on both sides.
    assert_ne!(my_addr, peer_addr);
    let key_id = if my_addr < peer_addr { 0 } else { 1 };
    debug!("key_id = {}", key_id);

    // derive a symmetric key for encryption of messages
    let key = SymmetricKey::new(preshared_key, key_id)?;
    let key = Arc::new(Mutex::new(key));

    let spec = Spec {
        format: Format::S16NE,
        channels: config.audio_ch,
        rate: config.audio_rate,
    };
    assert!(spec.is_valid());

    let traffic = Arc::new(TrafficMeter::new());

    // spawn threads
    spawn_command_thread(sock.clone(), key.clone(), peer_addr, traffic.clone());
    spawn_pulseaudio_input_thread(
        sock.clone(),
        key.clone(),
        peer_addr,
        config.clone(),
        traffic.clone(),
    );
    spawn_heartbeat_thread(sock.clone(), key.clone(), peer_addr);

    let output = Simple::new(
        None,                // Use the default server
        "p2p-voice-chat",    // Our application’s name
        Direction::Playback, // We want a playback stream
        None,                // Use the default device
        "output",            // Description of our stream
        &spec,               // Our sample format
        None,                // Use default channel map
        None,                // Use default buffering attributes
    )
    .unwrap_or_else(|e| panic!("pulseaudio error: {:?}", e.to_string()));

    let ch = match config.audio_ch {
        1 => opus::Channels::Mono,
        2 => opus::Channels::Stereo,
        _ => panic!("unsupported channel"),
    };
    let mut opus = opus::Decoder::new(spec.rate, ch).unwrap();
    let mut decoded = vec![0i16; config.buf_size / 2];
    let decoded = {
        let (p, buf, s) = unsafe { decoded.align_to_mut::<u8>() };
        assert!(p.is_empty() && s.is_empty());
        assert!(buf.len() >= config.buf_size);
        &mut buf[..config.buf_size]
    };

    'process_message: loop {
        let (enc_msg, src) = match recv_from::<Sealed<Message>>(&sock) {
            Ok(ok) => ok,
            Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::WouldBlock => {
                debug!("timeout");
                continue 'process_message;
            }
            Err(err) => {
                error!("{}", err);
                sleep(Duration::from_secs(1));
                continue 'process_message;
            }
        };

        if src != peer_addr {
            error!("message from other than the expected peer. ignored.");
            continue 'process_message;
        }

        // decrypt received message
        let msg = {
            let key = key.lock().unwrap();
            match key.decrypt(enc_msg) {
                Ok(msg) => msg,
                Err(err) => {
                    error!("invalid message: {}", err);
                    continue 'process_message;
                }
            }
        };

        match msg {
            Message::Heartbeat => {
                debug!("Heatbeat from {}", src);
            }
            Message::Opus(data) => {
                traffic.received_bytes(data.len());
                let (_, decoded, _) = unsafe { decoded.align_to_mut::<i16>() };
                let sz = opus.decode(&data, decoded, false).unwrap();
                let (_, pcm, _) = unsafe { decoded[..sz].align_to_mut::<u8>() };

                output.write(&pcm).unwrap();
            }
            Message::Text(text) => {
                println!("text message: {}", text);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("invalid argument: {}", .0)]
    InvalidArg(String),
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct InvitationToken(SocketAddr, Vec<u8>);

fn start(matches: clap::ArgMatches) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let local_port = matches
        .value_of("local-port")
        .unwrap_or("0")
        .parse::<u16>()
        .map_err(|e| AppError::InvalidArg(format!("invalid <local-port>: {}", e)))?;

    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, local_port))?;
    info!("socket port = {}", sock.local_addr().unwrap().port());

    let config = {
        let audio_ch = matches
            .value_of("audio-ch")
            .unwrap_or("1")
            .parse::<u8>()
            .map_err(|e| AppError::InvalidArg(format!("invalid <audio-ch>: {}", e)))?;
        if !(audio_ch == 1 || audio_ch == 2) {
            return Err(AppError::InvalidArg("<audio-ch> must be 1 or 2".into()).into());
        }

        let audio_rate = matches
            .value_of("audio-rate")
            .unwrap_or("24000")
            .parse::<u32>()
            .map_err(|e| AppError::InvalidArg(format!("invalid <audio-rate>: {}", e)))?;

        let buf_size = matches
            .value_of("buffer-size")
            .unwrap_or("960")
            .parse::<usize>()
            .map_err(|e| AppError::InvalidArg(format!("invalid <buffer-size>: {}", e)))?;
        if buf_size % 2 != 0 {
            return Err(AppError::InvalidArg("<buffer-size> must be multiple of 2".into()).into());
        }

        Config {
            audio_ch,
            audio_rate,
            buf_size,
        }
    };

    let psk;
    let (my_addr, peer_addr) = match matches.subcommand() {
        ("manual", Some(matches)) | ("invite", Some(matches)) => {
            let addr = matches.value_of("server-address").expect("required arg");
            let port = matches.value_of("server-port").expect("required arg");
            let port = port
                .parse::<u16>()
                .map_err(|e| AppError::InvalidArg(format!("invalid <server-port>: {}", e)))?;
            let server_sockaddr = (addr, port).to_socket_addrs()?.next().unwrap();

            psk = matches
                .value_of("preshared-key")
                .map(|psk| psk.as_bytes().to_vec())
                .unwrap_or_else(|| {
                    let mut key = vec![0u8; 8];
                    let mut rng = rand::thread_rng();
                    rng.fill(key.as_mut_slice());
                    key
                });

            let token = InvitationToken(server_sockaddr, psk.clone());
            let token_bytes = serde_cbor::to_vec(&token)?;
            println!("invitation-token: {}", base64::encode(&token_bytes));

            get_peer_addr(&sock, server_sockaddr, &psk)?
        }

        ("join", Some(matches)) => {
            let token = matches.value_of("invitation-token").expect("required arg");
            let token_bytes = base64::decode(token)?;
            let token: InvitationToken = serde_cbor::from_slice(&token_bytes)?;
            psk = token.1;
            get_peer_addr(&sock, token.0, &psk)?
        }

        ("lan", Some(matches)) => {
            println!("Specify peer's address and port number (e.g. 127.0.0.1:10001)");
            print!("address:port > ");
            std::io::stdout().flush()?;

            let mut peer_addr = String::new();
            std::io::stdin().read_line(&mut peer_addr)?;
            let peer_addr = peer_addr.trim().to_socket_addrs()?.next().unwrap();
            let my_addr = sock.local_addr()?;

            psk = matches
                .value_of("preshared-key")
                .expect("required arg")
                .as_bytes()
                .to_vec();

            (my_addr, peer_addr)
        }

        (cmd, _) => {
            return Err(AppError::InvalidArg(format!("unknown subcommand {:?}", cmd)).into())
        }
    };

    // start voice chating
    voice_chat(sock, my_addr, peer_addr, &psk, config)?;
    Ok(())
}

fn main() {
    env_logger::init();

    use clap::{App, Arg, SubCommand};
    let matches = App::new("denwa")
        .version(env!("CARGO_PKG_VERSION"))
        .author("algon-320 <algon.0320@mail.com>")
        .subcommand(
            SubCommand::with_name("manual")
                .arg(
                    Arg::with_name("server-address")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("server-port")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("preshared-key")
                        .takes_value(true)
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("invite")
                .arg(
                    Arg::with_name("server-address")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("server-port")
                        .takes_value(true)
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("join").arg(
                Arg::with_name("invitation-token")
                    .takes_value(true)
                    .required(true),
            ),
        )
        .subcommand(
            SubCommand::with_name("lan").arg(
                Arg::with_name("preshared-key")
                    .takes_value(true)
                    .required(true),
            ),
        )
        .arg(
            Arg::with_name("audio-ch")
                .long("ch")
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::with_name("audio-rate")
                .long("rate")
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::with_name("buffer-size")
                .long("bufsize")
                .takes_value(true)
                .required(false),
        )
        .arg(
            Arg::with_name("local-port")
                .long("local-port")
                .takes_value(true)
                .required(false),
        )
        .get_matches();

    match start(matches) {
        Ok(()) => {}
        Err(err) => {
            error!("{}", err);
        }
    }
}
