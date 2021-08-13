mod traffic_meter;

use std::io::prelude::*;
use std::io::{stdin, stdout};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

use log::{debug, error, info};
use p2p_handshake::{
    client::get_peer_addr,
    crypto::{Sealed, SymmetricKey},
    error::{Error, Result},
    message::{recv_from, send_to},
};
use pulse::{
    sample::{Format, Spec},
    stream::Direction,
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
    println!("['?' to show available commands]");
    spawn(move || {
        || -> Result<()> {
            loop {
                print!("command >>> ");
                stdout().flush().unwrap();

                let mut buffer = String::new();
                stdin().read_line(&mut buffer)?;

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

    let record = simple_pulse::Simple::new(
        None,              // Use the default server
        "denwa",           // Our application’s name
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

    let output = simple_pulse::Simple::new(
        None,                // Use the default server
        "denwa",             // Our application’s name
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

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct InvitationToken {
    addr: SocketAddr,
    psk: Vec<u8>,
}

fn random_psk(len: usize) -> Vec<u8> {
    use rand::{thread_rng, Rng};
    let mut key = vec![0u8; len];
    let mut rng = thread_rng();
    rng.fill(key.as_mut_slice());
    key
}

fn start(matches: clap::ArgMatches) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use clap::value_t;

    let config = {
        let audio_ch = value_t!(matches, "audio-ch", u8).unwrap();
        let audio_rate = value_t!(matches, "audio-rate", u32).unwrap();
        let buf_size = value_t!(matches, "buffer-size", usize).unwrap();
        Config {
            audio_ch,
            audio_rate,
            buf_size,
        }
    };

    match matches.subcommand() {
        ("wait", Some(matches)) => {
            let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
            info!("socket local address = {:?}", sock.local_addr().unwrap());

            let addr = matches.value_of("server-address").expect("required arg");
            let port = value_t!(matches, "server-port", u16).expect("required arg");
            let server_sockaddr = (addr, port).to_socket_addrs()?.next().unwrap();

            let psk = matches
                .value_of("preshared-key")
                .map(|psk| psk.as_bytes().to_vec())
                .unwrap_or_else(|| random_psk(8));

            let token = InvitationToken {
                addr: server_sockaddr,
                psk: psk.clone(),
            };
            let token_bytes = serde_cbor::to_vec(&token)?;
            println!("invitation-token: {}", base64::encode(&token_bytes));

            let (my_addr, peer_addr) = get_peer_addr(&sock, server_sockaddr, &psk)?;
            voice_chat(sock, my_addr, peer_addr, &psk, config)?;
            Ok(())
        }

        ("join", Some(matches)) => {
            let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
            info!("socket local address = {:?}", sock.local_addr().unwrap());

            let token = matches.value_of("invitation-token").expect("required arg");
            let token_bytes = base64::decode(token)?;
            let token: InvitationToken = serde_cbor::from_slice(&token_bytes)?;
            let (my_addr, peer_addr) = get_peer_addr(&sock, token.addr, &token.psk)?;

            voice_chat(sock, my_addr, peer_addr, &token.psk, config)?;
            Ok(())
        }

        ("lan", Some(matches)) => {
            let local_port = value_t!(matches, "local-port", u16).unwrap_or(0);
            let bind_addr = value_t!(matches, "bind-address", Ipv4Addr).unwrap();
            let sock = UdpSocket::bind((bind_addr, local_port))?;
            info!("socket local address = {:?}", sock.local_addr().unwrap());

            println!("Specify peer's address and port number (e.g. 127.0.0.1:10001)");
            print!("address:port > ");
            stdout().flush()?;

            let psk = matches
                .value_of("preshared-key")
                .map(|psk| psk.as_bytes().to_vec())
                .unwrap_or_else(|| random_psk(8));

            let peer_addr = {
                let mut peer_addr = String::new();
                stdin().read_line(&mut peer_addr)?;
                peer_addr.trim().to_socket_addrs()?.next().unwrap()
            };

            // FIXME: use of 0.0.0.0 will lead to insecure channel
            let my_addr = sock.local_addr()?;

            voice_chat(sock, my_addr, peer_addr, &psk, config)?;

            Ok(())
        }

        (cmd, _) => Err(format!("unknown subcommand {:?}", cmd).into()),
    }
}

fn main() {
    env_logger::init();

    use clap::{App, Arg, SubCommand};
    let matches = App::new("denwa")
        .version(env!("CARGO_PKG_VERSION"))
        .author("algon-320 <algon.0320@mail.com>")
        .subcommand(
            SubCommand::with_name("wait")
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
                        .long("psk")
                        .takes_value(true),
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
            SubCommand::with_name("lan")
                .arg(
                    Arg::with_name("bind-address")
                        .long("bind-address")
                        .short("a")
                        .takes_value(true)
                        .default_value("127.0.0.1"),
                )
                .arg(
                    Arg::with_name("local-port")
                        .long("local-port")
                        .short("p")
                        .takes_value(true),
                )
                .arg(
                    Arg::with_name("preshared-key")
                        .long("psk")
                        .takes_value(true),
                ),
        )
        .arg(
            Arg::with_name("audio-ch")
                .long("ch")
                .takes_value(true)
                .possible_values(&["1", "2"])
                .default_value("1"),
        )
        .arg(
            Arg::with_name("audio-rate")
                .long("rate")
                .takes_value(true)
                .possible_values(&["12000", "24000", "48000"])
                .default_value("24000"),
        )
        .arg(
            Arg::with_name("buffer-size")
                .long("bufsize")
                .takes_value(true)
                .default_value("960"),
        )
        .get_matches();

    match start(matches) {
        Ok(()) => {}
        Err(err) => {
            error!("{}", err);
        }
    }
}
