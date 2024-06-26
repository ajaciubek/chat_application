use libp2p::{
    floodsub::{Floodsub, FloodsubEvent, Topic},
    identity,
    mdns::{Mdns, MdnsEvent},
    swarm::NetworkBehaviourEventProcess,
    NetworkBehaviour, PeerId,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub static KEYS: Lazy<identity::Keypair> = Lazy::new(identity::Keypair::generate_ed25519);
pub static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));
pub static CHAT_TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("chat"));

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
struct IntroduceMyself {
    name: String,
    receiver: String,
}

#[derive(NetworkBehaviour)]
pub struct AppBehaviour {
    pub floodsub: Floodsub,
    pub mdns: Mdns,
    #[behaviour(ignore)]
    connected: HashMap<String, String>,
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
                    println!("{} is in chat room", resp.name);
                    self.connected.insert(msg.source.to_string(), resp.name);
                }
            } else if let Ok(resp) = serde_json::from_slice::<SayHello>(&msg.data) {
                if resp.hello {
                    println!("{} has joined the chat", resp.name);
                    self.connected.insert(msg.source.to_string(), resp.name);
                    self.introduce(self.name.clone(), msg.source.to_string());
                } else {
                    println!("{} has left the chat", resp.name);
                    self.connected.remove(&msg.source.to_string());
                }
            }
        }
    }
}

impl AppBehaviour {
    pub async fn new(name: String) -> Self {
        let mut behaviour = AppBehaviour {
            floodsub: Floodsub::new(*PEER_ID),
            mdns: Mdns::new(Default::default())
                .await
                .expect("cannot create mdns"),
            connected: HashMap::new(),
            name,
        };
        behaviour.floodsub.subscribe(CHAT_TOPIC.clone());
        behaviour
    }

    pub fn say_hello(&mut self, name: String, hello: bool) {
        self.send(SayHello { name, hello });
    }

    pub fn chat(&mut self, message: String) {
        self.send(ChatMessage { message });
    }

    pub fn introduce(&mut self, name: String, receiver: String) {
        self.send(IntroduceMyself { name, receiver });
    }

    fn send(&mut self, msg: impl serde::ser::Serialize) {
        let json = serde_json::to_string(&msg).expect("can jsonify response");
        self.floodsub.publish(CHAT_TOPIC.clone(), json.as_bytes());
    }
}
