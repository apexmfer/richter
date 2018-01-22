// Copyright © 2017 Cormac O'Brien
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::error::Error;
use std::fmt;
use std::io::BufReader;
use std::net::ToSocketAddrs;

use bsp;
use model::Model;
use net;
use net::BlockingMode;
use net::ColorShift;
use net::GameType;
use net::IntermissionKind;
use net::ItemFlags;
use net::NetError;
use net::PlayerColor;
use net::QSocket;
use net::ServerCmd;
use net::ServerCmdPrint;
use net::ServerCmdServerInfo;
use net::connect::CONNECT_PROTOCOL_VERSION;
use net::connect::ConnectSocket;
use net::connect::Request;
use net::connect::Response;
use pak::Pak;
use sound::Sound;

use cgmath::Deg;
use cgmath::Vector3;
use cgmath::Zero;
use chrono::Duration;

// connections are tried 3 times, see
// https://github.com/id-Software/Quake/blob/master/WinQuake/net_dgrm.c#L1248
const MAX_CONNECT_ATTEMPTS: usize = 3;

const MAX_STATS: usize = 32;

#[derive(Debug)]
pub enum ClientError {
    Io(::std::io::Error),
    Net(NetError),
    Other(String),
}

impl ClientError {
    pub fn with_msg<S>(msg: S) -> Self
    where
        S: AsRef<str>,
    {
        ClientError::Other(msg.as_ref().to_owned())
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ClientError::Io(ref err) => {
                write!(f, "I/O error: ")?;
                err.fmt(f)
            }
            ClientError::Net(ref err) => {
                write!(f, "Network error: ")?;
                err.fmt(f)
            }
            ClientError::Other(ref msg) => write!(f, "{}", msg),
        }
    }
}

impl Error for ClientError {
    fn description(&self) -> &str {
        match *self {
            ClientError::Io(ref err) => err.description(),
            ClientError::Net(ref err) => err.description(),
            ClientError::Other(ref msg) => &msg,
        }
    }
}

impl From<::std::io::Error> for ClientError {
    fn from(error: ::std::io::Error) -> Self {
        ClientError::Io(error)
    }
}

impl From<NetError> for ClientError {
    fn from(error: NetError) -> Self {
        ClientError::Net(error)
    }
}

struct ServerInfo {
    max_clients: u8,
    game_type: GameType,
}

struct ClientView {
    lerp_view_angles: [Vector3<Deg<f32>>; 2],
    view_angles: Vector3<Deg<f32>>,
    punch_angle: Vector3<Deg<f32>>,
    view_height: f32,
}

struct ScoreboardEntry {
    name: String,
    join_time: Duration,
    frags: i32,
    colors: PlayerColor,
    // translations: [u8; VID_GRADES],
}

struct ClientState {
    move_msg_count: usize,
    // cmd: MoveCmd,
    stats: [i32; MAX_STATS],
    items: ItemFlags,
    item_get_time: [f32; 32],
    face_anim_time: f32,
    color_shifts: [ColorShift; 4],
    prev_color_shifts: [ColorShift; 4],

    view: ClientView,

    m_velocity: [Vector3<f32>; 2],
    velocity: Vector3<f32>,

    ideal_pitch: Deg<f32>,
    pitch_velocity: f32,
    no_drift: bool,
    drift_move: f32,
    last_stop: f64,

    paused: bool,
    on_ground: bool,
    in_water: bool,

    intermission: IntermissionKind,
    completed_time: Duration,

    m_time: [Duration; 2],
    time: Duration,
    old_time: Duration,

    last_received_message: f32,

    model_precache: Vec<Model>,
    // sound_precache: Vec<Sfx>,
    level_name: String,
    view_ent: usize,

    server_info: ServerInfo,

    worldmodel: Model,
}

impl ClientState {
    /*
    pub fn new() -> ClientState {
        ClientState {
            move_msg_count: 0,
            // cmd: MoveCmd::new(),
            stats: [0; MAX_STATS],
            items: ItemFlags::empty(),
            item_get_time: [f32; 32],
            face_anim_time: f32,
            color_shifts: [
                ColorShift::new(),
                ColorShift::new(),
                ColorShift::new(),
                ColorShift::new(),
            ],
            prev_color_shifts: [
                ColorShift::new(),
                ColorShift::new(),
                ColorShift::new(),
                ColorShift::new(),
            ],

            m_view_angles: [
                Vector3::new(Deg::zero(), Deg::zero(), Deg::zero()),
                Vector3::new(Deg::zero(), Deg::zero(), Deg::zero()),
            ],

            view_angles: Vector3::new(Deg::zero(), Deg::zero(), Deg::zero()),

            m_velocity: [
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(0.0, 0.0, 0.0),
            ],

            velocity: Vector3::new(0.0, 0.0, 0.0),

            punch_angle: Vector3::new(Deg::zero(), Deg::zero(), Deg::zero()),
            ideal_pitch: Deg::zero(),
            pitch_velocity: 0.0,
            no_drift: false,
            drift_move: 0.0,
            last_stop: 0.0,

            view_height: 0.0,

            paused: false,
            on_ground: false,
            in_water: false,

            intermission: IntermissionKind::None,
            completed_time: Duration::zero(),

            m_time: [Duration::zero(), Duration::zero()],
            time: Duration::zero(),
            old_time: Duration::zero(),

            last_received_message: 0.0,

            model_precache: Vec::new(),

            level_name: String::new(),
            view_ent: 0,
            max_clients: 0,
            game_type: GameType::CoOp,

            worldmodel: Model::none(),
        }
    }
    */
}

pub struct Client {
    qsock: QSocket,
}

impl Client {
    pub fn connect<A>(server_addrs: A, pak: &Pak) -> Result<Client, ClientError>
    where
        A: ToSocketAddrs,
    {
        let mut con_sock = ConnectSocket::bind("0.0.0.0:0")?;
        let server_addr = server_addrs.to_socket_addrs().unwrap().next().unwrap();

        let mut response = None;

        for attempt in 0..MAX_CONNECT_ATTEMPTS {
            println!(
                "Connecting...(attempt {} of {})",
                attempt + 1,
                MAX_CONNECT_ATTEMPTS
            );
            con_sock.send_request(
                Request::connect(
                    net::GAME_NAME,
                    CONNECT_PROTOCOL_VERSION,
                ),
                server_addr,
            )?;

            // TODO: get rid of magic constant (2.5 seconds wait time for response)
            match con_sock.recv_response(Some(Duration::milliseconds(2500))) {
                Err(err) => {
                    match err {
                        // if the message is invalid, log it but don't quit
                        NetError::InvalidData(msg) => error!("{}", msg),

                        // other errors are fatal
                        _ => return Err(ClientError::from(err)),
                    }
                }

                Ok(opt) => {
                    if let Some((resp, remote)) = opt {
                        // if this response came from the right server, we're done
                        if remote == server_addr {
                            response = Some(resp);
                            break;
                        }
                    }
                }
            }
        }

        // make sure we actually got a response
        // TODO: specific error for this. shouldn't be fatal.
        if response.is_none() {
            return Err(ClientError::with_msg("No response"));
        }

        // we can unwrap this because we just checked it
        let port = match response.unwrap() {
            // if the server accepted our connect request, make sure the port number makes sense
            Response::Accept(accept) => {
                if accept.port < 0 || accept.port > ::std::u16::MAX as i32 {
                    return Err(ClientError::with_msg(format!("Invalid port number")));
                }

                println!("Connection accepted on port {}", accept.port);
                accept.port as u16
            }

            // our request was rejected. TODO: this error shouldn't be fatal.
            Response::Reject(reject) => {
                return Err(ClientError::with_msg(
                    format!("Connection rejected: {}", reject.message),
                ))
            }

            // the server sent back a response that doesn't make sense here (i.e. something other
            // than an Accept or Reject).
            // TODO: more specific error. this shouldn't be fatal.
            _ => return Err(ClientError::with_msg("Invalid connect response")),
        };

        let mut new_addr = server_addr;
        new_addr.set_port(port);

        // we're done with the connection socket, so turn it into a QSocket with the new address
        let mut qsock = con_sock.into_qsocket(new_addr);

        Ok(Client { qsock })
    }

    pub fn parse_server_msg(&mut self, block: BlockingMode, pak: &Pak) -> Result<(), ClientError> {
        let msg = self.qsock.recv_msg(block)?;

        // no data available at this time
        if msg.is_empty() {
            return Ok(());
        }

        let mut reader = BufReader::new(msg.as_slice());

        while let Some(cmd) = ServerCmd::read_cmd(&mut reader)? {
            match cmd {
                ServerCmd::NoOp => (),
                ServerCmd::Print(print_cmd) => {
                    // TODO: print to in-game console
                    println!("{}", print_cmd.text);
                }
                ServerCmd::ServerInfo(server_info) => self.update_server_info(server_info, pak)?,
                x => {
                    debug!("{:?}", x);
                    unimplemented!();
                }
            }
        }

        Ok(())
    }

    fn update_server_info(
        &mut self,
        server_info_cmd: ServerCmdServerInfo,
        pak: &Pak,
    ) -> Result<(), ClientError> {
        // TODO: wipe client state

        if server_info_cmd.protocol_version != net::PROTOCOL_VERSION as i32 {
            return Err(ClientError::with_msg(format!(
                "Incompatible protocol version (got {}, should be {})",
                server_info_cmd.protocol_version,
                net::PROTOCOL_VERSION
            )));
        }

        // TODO: print sign-on message to in-game console
        println!("{}", server_info_cmd.message);

        // first model and first sound are null

        let mut models = vec![Model::none()];
        models.push(Model::none());

        // TODO: validate submodel names
        for mod_name in server_info_cmd.model_precache {
            if mod_name.ends_with(".bsp") {
                let bsp_data = match pak.open(&mod_name) {
                    Some(b) => b,
                    None => {
                        return Err(ClientError::with_msg(
                            format!("Model not found in pak archive: {}", mod_name),
                        ))
                    }
                };

                let (mut brush_models, _) = bsp::load(bsp_data).unwrap();
                models.append(&mut brush_models);
            } else if !mod_name.starts_with("*") {
                debug!("Loading model {}", mod_name);
                models.push(Model::load(pak, mod_name));
            }
        }

        let mut sounds = vec![Sound::silent()];
        for snd_name in server_info_cmd.sound_precache {
            debug!("Loading sound {}", snd_name);
            sounds.push(Sound::load(pak, snd_name).unwrap());
        }

        let server_info = ServerInfo {
            max_clients: server_info_cmd.max_clients,
            game_type: server_info_cmd.game_type,
        };

        unimplemented!();
    }
}
