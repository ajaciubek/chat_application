use clap::Parser;
use ctrlc;
use std::collections::HashMap;
use std::process;
use std::thread;
use std::time::Duration;

use tokio::{
    io::{stdin, AsyncBufReadExt, BufReader},
    select, spawn,
    sync::mpsc,
    time::sleep,
};

use libp2p::{
    core::upgrade,
    floodsub::{Floodsub, FloodsubEvent, Topic},
    futures::StreamExt,
    identity,
    mdns::{Mdns, MdnsEvent},
    mplex,
    noise::{Keypair, NoiseConfig, X25519Spec},
    swarm::NetworkBehaviourEventProcess,
    swarm::{Swarm, SwarmBuilder},
    tcp::TokioTcpConfig,
    NetworkBehaviour, PeerId, Transport,
};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

pub static KEYS: Lazy<identity::Keypair> = Lazy::new(identity::Keypair::generate_ed25519);
pub static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));
pub static CHAT_TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("chat"));

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Name of the person to greet
    #[arg(short, long)]
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SayHello {
    name: String,
    hello: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntroduceMyself {
    name: String,
    receiver: String,
}

pub enum EventType {
    Input(String),
    Init,
    Introduction(IntroduceMyself),
    Exit,
}

#[derive(NetworkBehaviour)]
pub struct AppBehaviour {
    pub floodsub: Floodsub,
    pub mdns: Mdns,
    #[behaviour(ignore)]
    connected: HashMap<String, String>,
    #[behaviour(ignore)]
    reponse_sender: mpsc::UnboundedSender<IntroduceMyself>,
    #[behaviour(ignore)]
    name: String,
}

impl NetworkBehaviourEventProcess<MdnsEvent> for AppBehaviour {
    fn inject_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(discovered_list) => {
                for (peer, _addr) in discovered_list {
                    self.floodsub.add_node_to_partial_view(peer);
                }
            }
            MdnsEvent::Expired(expired_list) => {
                for (peer, _addr) in expired_list {
                    if !self.mdns.has_node(&peer) {
                        self.floodsub.remove_node_from_partial_view(&peer);
                    }
                }
            }
        }
    }
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for AppBehaviour {
    fn inject_event(&mut self, event: FloodsubEvent) {
        if let FloodsubEvent::Message(msg) = event {
            if let Ok(resp) = serde_json::from_slice::<ChatMessage>(&msg.data) {
                if let Some(author) = self.connected.get(&msg.source.to_string()) {
                    println!("{}: {}", author, resp.message);
                } else {
                    println!("Unknown: {}", resp.message);
                };
            } else if let Ok(resp) = serde_json::from_slice::<IntroduceMyself>(&msg.data) {
                if resp.receiver == (*PEER_ID).to_string() {
                    self.connected.insert(msg.source.to_string(), resp.name);
                }
            } else if let Ok(resp) = serde_json::from_slice::<SayHello>(&msg.data) {
                if resp.hello {
                    println!("{} has joined the chat", resp.name);
                    self.connected.insert(msg.source.to_string(), resp.name);
                    if let Err(e) = self.reponse_sender.send(IntroduceMyself {
                        name: self.name.clone(),
                        receiver: msg.source.to_string(),
                    }) {
                        print!("Error sending reposnse {}", e);
                    }
                } else {
                    println!("{} has left the chat", resp.name);
                    self.connected.remove(&msg.source.to_string());
                }
            }
        }
    }
}

impl AppBehaviour {
    pub async fn new(reponse_sender: mpsc::UnboundedSender<IntroduceMyself>, name: String) -> Self {
        let mut behaviour = AppBehaviour {
            floodsub: Floodsub::new(*PEER_ID),
            mdns: Mdns::new(Default::default())
                .await
                .expect("cannot create mdns"),
            connected: HashMap::new(),
            reponse_sender,
            name,
        };
        behaviour.floodsub.subscribe(CHAT_TOPIC.clone());
        behaviour
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // println!("Hello {}", *PEER_ID);
    let (init_sender, mut init_rcv) = mpsc::unbounded_channel();
    let (response_sender, mut response_rcv) = mpsc::unbounded_channel();
    let (exit_sender, mut exit_rcv) = mpsc::unbounded_channel();

    ctrlc::set_handler(move || {
        exit_sender.send(true).expect("could not send msg");
        thread::sleep(Duration::from_millis(500));
        process::exit(0x0);
    })
    .expect("Error setting Ctrl-C handler");

    let _init_sender = init_sender.clone();
    let auth_keys = Keypair::<X25519Spec>::new()
        .into_authentic(&KEYS)
        .expect("can create auth keys");

    let transport = TokioTcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(NoiseConfig::xx(auth_keys).into_authenticated())
        .multiplex(mplex::MplexConfig::new())
        .boxed();

    let behaviour = AppBehaviour::new(response_sender, args.name.clone()).await;
    let mut swarm = SwarmBuilder::new(transport, behaviour, *PEER_ID)
        .executor(Box::new(|fut| {
            spawn(fut);
        }))
        .build();
    let mut stdin = BufReader::new(stdin()).lines();

    Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .expect("can get a local socket"),
    )
    .expect("swarm can be started");

    spawn(async move {
        sleep(Duration::from_secs(1)).await;
        init_sender.send(true).expect("can send init event");
    });

    loop {
        let evt = {
            select! {
                line = stdin.next_line() => {
                    Some(EventType::Input(line.expect("can get line").expect("can read line from sdin")))
                },
                response = response_rcv.recv() =>{
                    Some(EventType::Introduction(response.expect("response exist")))
                },
                _init = init_rcv.recv()=>{
                    Some(EventType::Init)
                },
                _exit = exit_rcv.recv()=>{
                    Some(EventType::Exit)
                },
                _event = swarm.select_next_some() =>{
                    None
                }
            }
        };

        if let Some(event) = evt {
            match event {
                EventType::Init => {
                    let msg = SayHello {
                        name: args.name.clone(),
                        hello: true,
                    };

                    let json = serde_json::to_string(&msg).expect("can jsonify response");
                    // println!("Sending SayHello {}", json);
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(CHAT_TOPIC.clone(), json.as_bytes());
                }
                EventType::Introduction(resp) => {
                    let json = serde_json::to_string(&resp).expect("can jsonify response");
                    // println!("Sending Introduction {}", json);
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(CHAT_TOPIC.clone(), json.as_bytes());
                }
                EventType::Input(line) => {
                    let msg = ChatMessage { message: line };
                    let json = serde_json::to_string(&msg).expect("can jsonify response");
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(CHAT_TOPIC.clone(), json.as_bytes());
                }
                EventType::Exit => {
                    let msg = SayHello {
                        name: args.name.clone(),
                        hello: false,
                    };

                    let json = serde_json::to_string(&msg).expect("can jsonify response");
                    // println!("Sending SayGoodbye {}", json);
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(CHAT_TOPIC.clone(), json.as_bytes());
                }
            }
        }
    }
}
