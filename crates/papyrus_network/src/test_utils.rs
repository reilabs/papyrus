use std::task::{Context, Poll};
use std::{io, iter};

use futures::future::BoxFuture;
use futures::{AsyncRead, AsyncWrite, FutureExt};
use libp2p::core::transport::memory::MemoryTransport;
use libp2p::core::transport::Transport;
use libp2p::core::upgrade::{InboundUpgrade, OutboundUpgrade, UpgradeInfo};
use libp2p::core::{multiaddr, upgrade, Endpoint};
use libp2p::identity::Keypair;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::handler::{ConnectionEvent, FullyNegotiatedInbound, FullyNegotiatedOutbound};
use libp2p::swarm::{
    ConnectionDenied,
    ConnectionHandler,
    ConnectionHandlerEvent,
    ConnectionId,
    FromSwarm,
    KeepAlive,
    NetworkBehaviour,
    PollParameters,
    Stream,
    StreamProtocol,
    SubstreamProtocol,
    SwarmBuilder,
    SwarmEvent,
    ToSwarm,
};
use libp2p::{noise, yamux, Multiaddr, PeerId, Swarm};
use rand::random;
use tokio_stream::StreamExt as TokioStreamExt;

pub(crate) fn create_swarm<BehaviourT: NetworkBehaviour>(
    behaviour: BehaviourT,
) -> (Swarm<BehaviourT>, Multiaddr) {
    let key_pair = Keypair::generate_ed25519();
    let public_key = key_pair.public();
    let transport = MemoryTransport::default()
        .upgrade(upgrade::Version::V1)
        .authenticate(noise::Config::new(&key_pair).unwrap())
        .multiplex(yamux::Config::default())
        .boxed();

    let peer_id = public_key.to_peer_id();
    let mut swarm = SwarmBuilder::without_executor(transport, behaviour, peer_id).build();

    // Using a random address because if two different tests use the same address simultaneously
    // they will fail.
    let listen_address: Multiaddr = multiaddr::Protocol::Memory(random::<u64>()).into();
    swarm.listen_on(listen_address.clone()).unwrap();
    (swarm, listen_address)
}

pub(crate) async fn get_connected_streams() -> (Stream, Stream) {
    let (mut dialer_swarm, _) = create_swarm(GetStreamBehaviour::default());
    let (listener_swarm, listener_address) = create_swarm(GetStreamBehaviour::default());
    dialer_swarm
        .dial(
            DialOpts::peer_id(*listener_swarm.local_peer_id())
                .addresses(vec![listener_address])
                .build(),
        )
        .unwrap();
    let merged_swarm = dialer_swarm.merge(listener_swarm);
    let mut filtered_swarm = TokioStreamExt::filter_map(merged_swarm, |event| {
        if let SwarmEvent::Behaviour(stream) = event { Some(stream) } else { None }
    });
    let result = (
        TokioStreamExt::next(&mut filtered_swarm).await.unwrap(),
        TokioStreamExt::next(&mut filtered_swarm).await.unwrap(),
    );
    tokio::task::spawn(async move {
        while TokioStreamExt::next(&mut filtered_swarm).await.is_some() {}
    });
    result
}

#[derive(Default)]
struct GetStreamBehaviour {
    stream: Option<Stream>,
}

impl NetworkBehaviour for GetStreamBehaviour {
    type ConnectionHandler = GetStreamHandler;
    type ToSwarm = Stream;

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        Ok(GetStreamHandler { request_outbound_session: false, stream: None })
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: Endpoint,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        Ok(GetStreamHandler { request_outbound_session: true, stream: None })
    }

    fn on_swarm_event(&mut self, _event: FromSwarm<'_, Self::ConnectionHandler>) {}

    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        stream: <Self::ConnectionHandler as ConnectionHandler>::ToBehaviour,
    ) {
        self.stream = Some(stream);
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
        _params: &mut impl PollParameters,
    ) -> Poll<ToSwarm<Self::ToSwarm, <Self::ConnectionHandler as ConnectionHandler>::FromBehaviour>>
    {
        if let Some(stream) = self.stream.take() {
            return Poll::Ready(ToSwarm::GenerateEvent(stream));
        }
        Poll::Pending
    }
}

struct GetStreamHandler {
    request_outbound_session: bool,
    stream: Option<Stream>,
}

impl ConnectionHandler for GetStreamHandler {
    type FromBehaviour = ();
    type ToBehaviour = Stream;
    type Error = io::Error;
    type InboundProtocol = GetStreamProtocol;
    type OutboundProtocol = GetStreamProtocol;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = ();

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(GetStreamProtocol, ())
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        KeepAlive::Yes
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<
            Self::OutboundProtocol,
            Self::OutboundOpenInfo,
            Self::ToBehaviour,
            Self::Error,
        >,
    > {
        if self.request_outbound_session {
            self.request_outbound_session = false;
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest {
                protocol: SubstreamProtocol::new(GetStreamProtocol, ()),
            });
        }
        if let Some(stream) = self.stream.take() {
            return Poll::Ready(ConnectionHandlerEvent::NotifyBehaviour(stream));
        }
        Poll::Pending
    }

    fn on_behaviour_event(&mut self, _event: Self::FromBehaviour) {}

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            '_,
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        match event {
            ConnectionEvent::FullyNegotiatedOutbound(FullyNegotiatedOutbound {
                protocol: stream,
                info: _,
            }) => self.stream = Some(stream),
            ConnectionEvent::FullyNegotiatedInbound(FullyNegotiatedInbound {
                protocol: stream,
                info: _,
            }) => self.stream = Some(stream),
            _ => {}
        }
    }
}

pub const PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/get_stream");

pub struct GetStreamProtocol;

impl UpgradeInfo for GetStreamProtocol {
    type Info = StreamProtocol;
    type InfoIter = iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        iter::once(PROTOCOL_NAME)
    }
}

impl OutboundUpgrade<Stream> for GetStreamProtocol
where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = Stream;
    type Error = ();
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn upgrade_outbound(self, stream: Stream, _: Self::Info) -> Self::Future {
        async move { Ok(stream) }.boxed()
    }
}

impl InboundUpgrade<Stream> for GetStreamProtocol
where
    Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = Stream;
    type Error = ();
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn upgrade_inbound(self, stream: Stream, _: Self::Info) -> Self::Future {
        async move { Ok(stream) }.boxed()
    }
}
