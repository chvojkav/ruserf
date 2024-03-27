use std::sync::Arc;

use memberlist_core::transport::Node;

use super::{
  JoinMessage, LeaveMessage, Member, PushPull, PushPullRef, QueryMessage, QueryResponseMessage,
  UserEventMessage,
};

#[cfg(feature = "encryption")]
use super::{KeyRequestMessage, KeyResponseMessage};

const LEAVE_MESSAGE_TAG: u8 = 0;
const JOIN_MESSAGE_TAG: u8 = 1;
const PUSH_PULL_MESSAGE_TAG: u8 = 2;
const USER_EVENT_MESSAGE_TAG: u8 = 3;
const QUERY_MESSAGE_TAG: u8 = 4;
const QUERY_RESPONSE_MESSAGE_TAG: u8 = 5;
const CONFLICT_RESPONSE_MESSAGE_TAG: u8 = 6;
const RELAY_MESSAGE_TAG: u8 = 7;
#[cfg(feature = "encryption")]
const KEY_REQUEST_MESSAGE_TAG: u8 = 253;
#[cfg(feature = "encryption")]
const KEY_RESPONSE_MESSAGE_TAG: u8 = 254;

#[derive(Debug, thiserror::Error)]
#[error("unknown message type byte: {0}")]
pub struct UnknownMessageType(u8);

impl TryFrom<u8> for MessageType {
  type Error = UnknownMessageType;

  fn try_from(value: u8) -> Result<Self, Self::Error> {
    Ok(match value {
      LEAVE_MESSAGE_TAG => Self::Leave,
      JOIN_MESSAGE_TAG => Self::Join,
      PUSH_PULL_MESSAGE_TAG => Self::PushPull,
      USER_EVENT_MESSAGE_TAG => Self::UserEvent,
      QUERY_MESSAGE_TAG => Self::Query,
      QUERY_RESPONSE_MESSAGE_TAG => Self::QueryResponse,
      CONFLICT_RESPONSE_MESSAGE_TAG => Self::ConflictResponse,
      RELAY_MESSAGE_TAG => Self::Relay,
      #[cfg(feature = "encryption")]
      KEY_REQUEST_MESSAGE_TAG => Self::KeyRequest,
      #[cfg(feature = "encryption")]
      KEY_RESPONSE_MESSAGE_TAG => Self::KeyResponse,
      _ => return Err(UnknownMessageType(value)),
    })
  }
}

impl From<MessageType> for u8 {
  fn from(value: MessageType) -> Self {
    value as u8
  }
}

/// The types of gossip messages Serf will send along
/// memberlist.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum MessageType {
  Leave = LEAVE_MESSAGE_TAG,
  Join = JOIN_MESSAGE_TAG,
  PushPull = PUSH_PULL_MESSAGE_TAG,
  UserEvent = USER_EVENT_MESSAGE_TAG,
  Query = QUERY_MESSAGE_TAG,
  QueryResponse = QUERY_RESPONSE_MESSAGE_TAG,
  ConflictResponse = CONFLICT_RESPONSE_MESSAGE_TAG,
  Relay = RELAY_MESSAGE_TAG,
  #[cfg(feature = "encryption")]
  KeyRequest = KEY_REQUEST_MESSAGE_TAG,
  #[cfg(feature = "encryption")]
  KeyResponse = KEY_RESPONSE_MESSAGE_TAG,
}

impl MessageType {
  pub const fn as_str(&self) -> &'static str {
    match self {
      Self::Leave => "leave",
      Self::Join => "join",
      Self::PushPull => "push pull",
      Self::UserEvent => "user event",
      Self::Query => "query",
      Self::QueryResponse => "query response",
      Self::ConflictResponse => "conflict response",
      #[cfg(feature = "encryption")]
      Self::KeyRequest => "key request",
      #[cfg(feature = "encryption")]
      Self::KeyResponse => "key response",
      Self::Relay => "relay",
    }
  }
}

/// Used to do a cheap reference to message reference conversion.
pub trait AsMessageRef<I, A> {
  /// Converts this type into a shared reference of the (usually inferred) input type.
  fn as_message_ref(&self) -> SerfMessageRef<I, A>;
}

/// The reference type of [`SerfMessage`].
#[derive(Debug)]
pub enum SerfMessageRef<'a, I, A> {
  Leave(&'a LeaveMessage<I, A>),
  Join(&'a JoinMessage<I, A>),
  PushPull(PushPullRef<'a, I, A>),
  UserEvent(&'a UserEventMessage),
  Query(&'a QueryMessage<I, A>),
  QueryResponse(&'a QueryResponseMessage<I, A>),
  ConflictResponse(&'a Member<I, A>),
  #[cfg(feature = "encryption")]
  KeyRequest(&'a KeyRequestMessage),
  #[cfg(feature = "encryption")]
  KeyResponse(&'a KeyResponseMessage),
  Relay,
}

impl<'a, I, A> Clone for SerfMessageRef<'a, I, A> {
  fn clone(&self) -> Self {
    *self
  }
}

impl<'a, I, A> Copy for SerfMessageRef<'a, I, A> {}

impl<'a, I, A> AsMessageRef<I, A> for SerfMessageRef<'a, I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    *self
  }
}

/// The types of gossip messages Serf will send along
/// showbiz.
#[derive(Debug, Clone)]
pub enum SerfMessage<I, A> {
  Leave(LeaveMessage<I, A>),
  Join(JoinMessage<I, A>),
  PushPull(PushPull<I, A>),
  UserEvent(UserEventMessage),
  Query(QueryMessage<I, A>),
  QueryResponse(QueryResponseMessage<I, A>),
  ConflictResponse(Arc<Member<I, A>>),
  #[cfg(feature = "encryption")]
  KeyRequest(KeyRequestMessage),
  #[cfg(feature = "encryption")]
  KeyResponse(KeyResponseMessage),
}

impl<'a, I, A> From<&'a SerfMessage<I, A>> for MessageType {
  fn from(msg: &'a SerfMessage<I, A>) -> Self {
    match msg {
      SerfMessage::Leave(_) => MessageType::Leave,
      SerfMessage::Join(_) => MessageType::Join,
      SerfMessage::PushPull(_) => MessageType::PushPull,
      SerfMessage::UserEvent(_) => MessageType::UserEvent,
      SerfMessage::Query(_) => MessageType::Query,
      SerfMessage::QueryResponse(_) => MessageType::QueryResponse,
      SerfMessage::ConflictResponse(_) => MessageType::ConflictResponse,
      #[cfg(feature = "encryption")]
      SerfMessage::KeyRequest(_) => MessageType::KeyRequest,
      #[cfg(feature = "encryption")]
      SerfMessage::KeyResponse(_) => MessageType::KeyResponse,
    }
  }
}

impl<I, A> AsMessageRef<I, A> for QueryMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::Query(self)
  }
}

impl<I, A> AsMessageRef<I, A> for QueryResponseMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::QueryResponse(self)
  }
}

impl<I, A> AsMessageRef<I, A> for JoinMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::Join(self)
  }
}

impl<I, A> AsMessageRef<I, A> for UserEventMessage {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::UserEvent(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a QueryMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::Query(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a QueryResponseMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::QueryResponse(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a JoinMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::Join(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for PushPullRef<'a, I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::PushPull(*self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a UserEventMessage {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::UserEvent(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a LeaveMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::Leave(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a Member<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::ConflictResponse(self)
  }
}

impl<'a, I, A> AsMessageRef<I, A> for &'a Arc<Member<I, A>> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::ConflictResponse(self)
  }
}

#[cfg(feature = "encryption")]
impl<'a, I, A> AsMessageRef<I, A> for &'a KeyRequestMessage {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::KeyRequest(self)
  }
}

#[cfg(feature = "encryption")]
impl<'a, I, A> AsMessageRef<I, A> for &'a KeyResponseMessage {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    SerfMessageRef::KeyResponse(self)
  }
}

impl<I, A> AsMessageRef<I, A> for SerfMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    match self {
      Self::Leave(l) => SerfMessageRef::Leave(l),
      Self::Join(j) => SerfMessageRef::Join(j),
      Self::PushPull(pp) => SerfMessageRef::PushPull(PushPullRef {
        ltime: pp.ltime,
        status_ltimes: &pp.status_ltimes,
        left_members: &pp.left_members,
        event_ltime: pp.event_ltime,
        events: &pp.events,
        query_ltime: pp.query_ltime,
      }),
      Self::UserEvent(u) => SerfMessageRef::UserEvent(u),
      Self::Query(q) => SerfMessageRef::Query(q),
      Self::QueryResponse(q) => SerfMessageRef::QueryResponse(q),
      Self::ConflictResponse(m) => SerfMessageRef::ConflictResponse(m),
      #[cfg(feature = "encryption")]
      Self::KeyRequest(kr) => SerfMessageRef::KeyRequest(kr),
      #[cfg(feature = "encryption")]
      Self::KeyResponse(kr) => SerfMessageRef::KeyResponse(kr),
    }
  }
}

impl<'b, I, A> AsMessageRef<I, A> for &'b SerfMessage<I, A> {
  fn as_message_ref(&self) -> SerfMessageRef<I, A> {
    match self {
      SerfMessage::Leave(l) => SerfMessageRef::Leave(l),
      SerfMessage::Join(j) => SerfMessageRef::Join(j),
      SerfMessage::PushPull(pp) => SerfMessageRef::PushPull(PushPullRef {
        ltime: pp.ltime,
        status_ltimes: &pp.status_ltimes,
        left_members: &pp.left_members,
        event_ltime: pp.event_ltime,
        events: &pp.events,
        query_ltime: pp.query_ltime,
      }),
      SerfMessage::UserEvent(u) => SerfMessageRef::UserEvent(u),
      SerfMessage::Query(q) => SerfMessageRef::Query(q),
      SerfMessage::QueryResponse(q) => SerfMessageRef::QueryResponse(q),
      SerfMessage::ConflictResponse(m) => SerfMessageRef::ConflictResponse(m),
      #[cfg(feature = "encryption")]
      SerfMessage::KeyRequest(kr) => SerfMessageRef::KeyRequest(kr),
      #[cfg(feature = "encryption")]
      SerfMessage::KeyResponse(kr) => SerfMessageRef::KeyResponse(kr),
    }
  }
}

impl<I, A> core::fmt::Display for SerfMessage<I, A> {
  fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    write!(f, "{}", self.as_str())
  }
}

impl<I, A> SerfMessage<I, A> {
  pub(crate) const fn as_str(&self) -> &'static str {
    match self {
      Self::Leave(_) => "leave",
      Self::Join(_) => "join",
      Self::PushPull(_) => "push pull",
      Self::UserEvent(_) => "user event",
      Self::Query(_) => "query",
      Self::QueryResponse(_) => "query response",
      Self::ConflictResponse(_) => "conflict response",
      #[cfg(feature = "encryption")]
      Self::KeyRequest(_) => "key request",
      #[cfg(feature = "encryption")]
      Self::KeyResponse(_) => "key response",
    }
  }
}

/// Used to store the end destination of a relayed message
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
#[repr(transparent)]
pub(crate) struct RelayHeader<I, A> {
  pub(crate) dest: Node<I, A>,
}
