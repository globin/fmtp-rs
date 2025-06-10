mod config;
mod connection;
mod event;
mod identifier;
mod message;
mod packet;

pub use config::{Config, ConnectionConfig, Role, Target};
pub use connection::{Connection, ConnectionContext, State};
pub use event::{Event, UserCommand};
pub use identifier::FmtpIdentifier;
pub use message::FmtpMessage;
pub use packet::{FmtpPacket, FmtpType};

use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ti(pub Instant);
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Tr(pub Instant);
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ts(pub Instant);

pub struct TxFut(Option<FmtpPacket>);
impl Future for TxFut {
    type Output = FmtpPacket;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(packet) = self.0.take() {
            Poll::Ready(packet)
        } else {
            Poll::Pending
        }
    }
}

pub struct RxFut(Option<FmtpMessage>);
impl Future for RxFut {
    type Output = FmtpMessage;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(packet) = self.0.take() {
            Poll::Ready(packet)
        } else {
            Poll::Pending
        }
    }
}
