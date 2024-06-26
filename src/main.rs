use chat_application::*;
use clap::Parser;
use ctrlc;
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
    futures::StreamExt,
    mplex,
    noise::{Keypair, NoiseConfig, X25519Spec},
    swarm::{Swarm, SwarmBuilder},
    tcp::TokioTcpConfig,
    Transport,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    name: String,
}

enum EventType {
    Init,
    Input(String),
    Exit,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // println!("Hello {}", *PEER_ID);
    let (init_sender, mut init_rcv) = mpsc::unbounded_channel();
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

    let behaviour = AppBehaviour::new(args.name.clone()).await;
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
                    swarm.behaviour_mut().say_hello(args.name.clone(), true);
                }
                EventType::Input(line) => {
                    swarm.behaviour_mut().chat(line);
                }
                EventType::Exit => {
                    swarm.behaviour_mut().say_hello(args.name.clone(), false);
                }
            }
        }
    }
}
