use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, ReadHalf, Result};
use tokio::net::{TcpSocket, UdpSocket};
use tokio::sync::mpsc;
use tokio_util::codec::{ Framed, FramedRead, LinesCodec, LinesCodecError};
use webrtc_ice::agent::Agent;
use webrtc_ice::agent::agent_config::AgentConfig;
use webrtc_ice::candidate::Candidate;
use webrtc_ice::Error;
use webrtc_ice::network_type::NetworkType;
use webrtc_ice::state::ConnectionState;
use webrtc_ice::udp_mux::{UDPMuxDefault, UDPMuxParams};
use webrtc_ice::udp_network::UDPNetwork;


#[tokio::main]
async fn main() -> Result<()> {

    let is_controlling = false;
    let use_mux = false;

    let remote_port = 4000;

    let ice_agent = create_ice_agent(is_controlling).await.unwrap();
    ice_agent.on_candidate(handle_candidate()).await;
    let (ice_done_tx, mut ice_done_rx) = mpsc::channel::<()>(1);

    ice_agent
        .on_connection_state_change(Box::new(move |c: ConnectionState| {
            println!("ICE Connection State has changed: {}", c);
            if c == ConnectionState::Failed {
                let _ = ice_done_tx.try_send(());
            }
            Box::pin(async move {})
        }))
        .await;
    // Get the local auth details and send to remote peer
    let (local_ufrag, local_pwd) = ice_agent.get_local_user_credentials().await;



    let tcp_socket = TcpSocket::new_v4().unwrap();
    let addr = "127.0.0.1:8080".parse().unwrap();
    // let _ = tcp_socket.set_reuseaddr(true);
    // let _ = tcp_socket.set_reuseport(true);
    let mut connection = tcp_socket.connect(addr).await.unwrap();
    let message_string = format!("{}:{}", local_ufrag, local_pwd);
    let message = message_string.as_bytes();
    // let message = b"Hey there!";
    // let mut framed_connection = Framed::new(connection, tokio_util::codec::LinesCodec::new());
    // let _ = framed_connection.send(message).await;
    let (mut stream, sink) = connection.split();
    let mut buf = [0;8];
    sink.writable().await?;
    sink.try_write(message).unwrap();
    println!("Listening on {:?}", addr);
    while let Ok(res) = stream.read(&mut buf).await {
        println!("{:?}", std::str::from_utf8(&buf));
    }
    println!("Finished");
    Ok(())

}



async fn create_ice_agent(is_controlling: bool) -> Result<Arc<Agent>> {
    let local_port = if is_controlling { 4000 } else { 4001};

    let udp_network = {
        let udp_socket = UdpSocket::bind(("0.0.0.0", local_port)).await?;
        let udp_mux = UDPMuxDefault::new(UDPMuxParams::new(udp_socket));
        UDPNetwork::Muxed(udp_mux)
    };
    let agent = Agent::new(AgentConfig {
        network_types: vec![NetworkType::Udp4],
        udp_network,
        ..Default::default()
    })
        .await.unwrap();
    let ice_agent = Arc::new(agent);
    Ok(ice_agent)
}


fn handle_candidate<'a>() -> Box<
    dyn FnMut(Option<Arc<dyn Candidate + Send + Sync>>) -> Pin<Box<dyn Future<Output = ()> + Send>>
    + Send
    + Sync,
> {
    Box::new(move |c: Option<Arc<dyn Candidate + Send + Sync>>| {
        Box::pin(async move {
            if let Some(c) = c {
                println!("posting remoteCandidate with {}", c.marshal());
            }
        })
    })
}
