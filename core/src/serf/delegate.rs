use crate::{
  broadcast::SerfBroadcast,
  delegate::{Delegate, TransformDelegate},
  error::{DelegateError, Error},
  types::{
    JoinMessage, LamportTime, LeaveMessage, Member, MemberStatus, MessageType, PushPullRef,
    SerfMessage, UserEventMessage,
  },
  Serf,
};

use std::{
  future::Future,
  sync::{Arc, OnceLock},
};

use atomic::{Atomic, Ordering};
use futures::Stream;
use indexmap::IndexSet;
use memberlist_core::{
  agnostic_lite::RuntimeLite,
  bytes::{Buf, BufMut, Bytes, BytesMut},
  delegate::{
    AliveDelegate, ConflictDelegate, Delegate as MemberlistDelegate, EventDelegate,
    MergeDelegate as MemberlistMergeDelegate, NodeDelegate, PingDelegate,
  },
  tracing,
  transport::{AddressResolver, Node, Transport},
  types::{DelegateVersion, Meta, NodeState, ProtocolVersion, SmallVec, State, TinyVec},
  CheapClone, META_MAX_SIZE,
};

// PingVersion is an internal version for the ping message, above the normal
// versioning we get from the protocol version. This enables small updates
// to the ping message without a full protocol bump.
const PING_VERSION: u8 = 1;

pub(crate) struct SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  serf: OnceLock<Serf<T, D>>,
  delegate: Option<D>,
}

impl<D, T> SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  pub(crate) fn new(d: Option<D>) -> Self {
    Self {
      serf: OnceLock::new(),
      delegate: d,
    }
  }

  pub(crate) fn delegate(&self) -> Option<&D> {
    self.delegate.as_ref()
  }

  pub(crate) fn store(&self, s: Serf<T, D>) {
    // No error, we never call this in parallel
    let _ = self.serf.set(s);
  }

  fn this(&self) -> &Serf<T, D> {
    self.serf.get().unwrap()
  }
}

impl<D, T> NodeDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  async fn node_meta(&self, limit: usize) -> Meta {
    match self.this().inner.opts.tags() {
      Some(tags) => {
        let encoded_len = <D as TransformDelegate>::tags_encoded_len(&tags);
        let limit = limit.min(Meta::MAX_SIZE);
        if encoded_len > limit {
          panic!(
            "node tags {:?} exceeds length limit of {} bytes",
            tags, limit
          );
        }

        let mut role_bytes = vec![0; encoded_len];
        match <D as TransformDelegate>::encode_tags(&tags, &mut role_bytes) {
          Ok(len) => {
            debug_assert_eq!(
              len, encoded_len,
              "expected encoded len {} mismatch the actual encoded len {}",
              encoded_len, len
            );

            if len > limit {
              panic!(
                "node tags {:?} exceeds length limit of {} bytes",
                tags, limit
              );
            }

            role_bytes.try_into().unwrap()
          }
          Err(e) => {
            tracing::error!(err=%e, "ruserf: failed to encode tags");
            Meta::empty()
          }
        }
      }
      None => Meta::empty(),
    }
  }

  async fn notify_message(&self, mut msg: Bytes) {
    // If we didn't actually receive any data, then ignore it.
    if msg.is_empty() {
      return;
    }

    #[cfg(feature = "metrics")]
    {
      metrics::histogram!(
        "ruserf.messages.received",
        self
          .this()
          .inner
          .opts
          .memberlist_options
          .metric_labels
          .iter()
      )
      .record(msg.len() as f64);
    }

    let this = self.this();
    let mut rebroadcast = None;
    let mut rebroadcast_queue = &this.inner.broadcasts;

    match MessageType::try_from(msg[0]) {
      Ok(ty) => match ty {
        MessageType::Leave => match <D as TransformDelegate>::decode_message(&msg[1..]) {
          Ok((_, l)) => {
            if let SerfMessage::Leave(l) = &l {
              tracing::debug!("ruserf: leave message",);
              rebroadcast = this.handle_node_leave_intent(l).await.then(|| msg.clone());
            } else {
              tracing::warn!("ruserf: receive unexpected message: {}", l.as_str());
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode message");
          }
        },
        MessageType::Join => match <D as TransformDelegate>::decode_message(&msg[1..]) {
          Ok((_, j)) => {
            if let SerfMessage::Join(j) = &j {
              tracing::debug!("ruserf: join message",);
              rebroadcast = this.handle_node_join_intent(j).await.then(|| msg.clone());
            } else {
              tracing::warn!("ruserf: receive unexpected message: {}", j.as_str());
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode message");
          }
        },
        MessageType::UserEvent => match <D as TransformDelegate>::decode_message(&msg[1..]) {
          Ok((_, ue)) => {
            if let SerfMessage::UserEvent(ue) = ue {
              tracing::debug!("ruserf: user event message",);
              rebroadcast = this.handle_user_event(ue).await.then(|| msg.clone());
              rebroadcast_queue = &this.inner.event_broadcasts;
            } else {
              tracing::warn!("ruserf: receive unexpected message: {}", ue.as_str());
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode message");
          }
        },
        MessageType::Query => match <D as TransformDelegate>::decode_message(&msg[1..]) {
          Ok((_, q)) => {
            if let SerfMessage::Query(q) = q {
              tracing::debug!("ruserf: query message",);
              rebroadcast = this.handle_query(q, None).await.then(|| msg.clone());
              rebroadcast_queue = &this.inner.query_broadcasts;
            } else {
              tracing::warn!("ruserf: receive unexpected message: {}", q.as_str());
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode message");
          }
        },
        MessageType::QueryResponse => match <D as TransformDelegate>::decode_message(&msg[1..]) {
          Ok((_, qr)) => {
            if let SerfMessage::QueryResponse(qr) = qr {
              tracing::debug!("ruserf: query response message",);
              this.handle_query_response(qr).await;
            } else {
              tracing::warn!("ruserf: receive unexpected message: {}", qr.as_str());
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode message");
          }
        },
        MessageType::Relay => match <D as TransformDelegate>::decode_node(&msg[1..]) {
          Ok((consumed, n)) => {
            tracing::debug!("ruserf: relay message",);
            tracing::debug!("ruserf: relaying response to node: {}", n);
            // + 1 for the message type byte
            msg.advance(consumed + 1);
            if let Err(e) = this.inner.memberlist.send(n.address(), msg.clone()).await {
              tracing::error!(err=%e, "ruserf: failed to forwarding message to {}", n);
            }
          }
          Err(e) => {
            tracing::warn!(err=%e, "ruserf: failed to decode relay destination");
          }
        },
        ty => {
          tracing::warn!("ruserf: receive unexpected message: {}", ty.as_str());
        }
      },
      Err(e) => {
        tracing::warn!(err=%e, "ruserf: receive unknown message type");
      }
    }

    if let Some(msg) = rebroadcast {
      rebroadcast_queue
        .queue_broadcast(SerfBroadcast {
          msg,
          notify_tx: None,
        })
        .await;
    }
  }

  async fn broadcast_messages<F>(
    &self,
    overhead: usize,
    limit: usize,
    encoded_len: F,
  ) -> TinyVec<Bytes>
  where
    F: Fn(Bytes) -> (usize, Bytes) + Send,
  {
    let this = self.this();
    let mut msgs = this.inner.broadcasts.get_broadcasts(overhead, limit).await;

    // Determine the bytes used already
    let mut bytes_used = 0;
    for msg in msgs.iter() {
      let (encoded_len, _) = encoded_len(msg.clone());
      bytes_used += encoded_len;
      #[cfg(feature = "metrics")]
      {
        metrics::histogram!(
          "ruserf.messages.sent",
          this.inner.opts.memberlist_options.metric_labels.iter()
        )
        .record(encoded_len as f64);
      }
    }

    // Get any additional query broadcasts
    let query_msgs = this
      .inner
      .query_broadcasts
      .get_broadcasts(overhead, limit - bytes_used)
      .await;
    for msg in query_msgs.iter() {
      let (encoded_len, _) = encoded_len(msg.clone());
      bytes_used += encoded_len;
      #[cfg(feature = "metrics")]
      {
        metrics::histogram!(
          "ruserf.messages.sent",
          this.inner.opts.memberlist_options.metric_labels.iter()
        )
        .record(encoded_len as f64);
      }
    }

    // Get any additional event broadcasts
    let event_msgs = this
      .inner
      .event_broadcasts
      .get_broadcasts(overhead, limit - bytes_used)
      .await;
    for msg in query_msgs.iter() {
      let (encoded_len, _) = encoded_len(msg.clone());
      bytes_used += encoded_len;
      #[cfg(feature = "metrics")]
      {
        metrics::histogram!(
          "ruserf.messages.sent",
          this.inner.opts.memberlist_options.metric_labels.iter()
        )
        .record(encoded_len as f64);
      }
    }

    msgs.extend(query_msgs);
    msgs.extend(event_msgs);
    msgs
  }

  async fn local_state(&self, _join: bool) -> Bytes {
    let this = self.this();
    let members = this.inner.members.read().await;
    let events = this.inner.event_core.read().await;

    // Create the message to send
    let status_ltimes = members
      .states
      .values()
      .map(|v| (v.member.node.cheap_clone(), v.status_time))
      .collect();
    let left_members = members
      .left_members
      .iter()
      .map(|v| v.member.node().cheap_clone())
      .collect::<IndexSet<Node<T::Id, <T::Resolver as AddressResolver>::ResolvedAddress>>>();
    let pp = PushPullRef {
      ltime: this.inner.clock.time(),
      status_ltimes: &status_ltimes,
      left_members: &left_members,
      event_ltime: this.inner.event_clock.time(),
      events: events.buffer.as_slice(),
      query_ltime: this.inner.query_clock.time(),
    };
    drop(members);

    let expected_encoded_len = <D as TransformDelegate>::message_encoded_len(pp);
    let mut buf = BytesMut::with_capacity(expected_encoded_len + 1); // +1 for the message type byte
    buf.put_u8(MessageType::PushPull as u8);
    buf.resize(expected_encoded_len + 1, 0);
    match <D as TransformDelegate>::encode_message(pp, &mut buf[1..]) {
      Ok(encoded_len) => {
        debug_assert_eq!(
          expected_encoded_len, encoded_len,
          "expected encoded len {} mismatch the actual encoded len {}",
          expected_encoded_len, encoded_len
        );
        buf.freeze()
      }
      Err(e) => {
        tracing::error!(err=%e, "ruserf: failed to encode local state");
        Bytes::new()
      }
    }
  }

  async fn merge_remote_state(&self, buf: Bytes, is_join: bool) {
    if buf.is_empty() {
      tracing::error!("ruserf: remote state is zero bytes");
      return;
    }

    // Check the message type
    let Ok(ty) = MessageType::try_from(buf[0]) else {
      tracing::error!("ruserf: remote state has bad type prefix {}", buf[0]);
      return;
    };

    // TODO: messageDropper
    match ty {
      MessageType::PushPull => {
        match <D as TransformDelegate>::decode_message(&buf[1..]) {
          Err(e) => {
            tracing::error!(err=%e, "ruserf: failed to decode remote state");
          }
          Ok((_, msg)) => {
            match msg {
              SerfMessage::PushPull(pp) => {
                let this = self.this();
                // Witness the Lamport clocks first.
                // We subtract 1 since no message with that clock has been sent yet
                if pp.ltime > LamportTime::ZERO {
                  this.inner.clock.witness(pp.ltime - LamportTime(1));
                }
                if pp.event_ltime > LamportTime::ZERO {
                  this
                    .inner
                    .event_clock
                    .witness(pp.event_ltime - LamportTime(1));
                }
                if pp.query_ltime > LamportTime::ZERO {
                  this
                    .inner
                    .query_clock
                    .witness(pp.query_ltime - LamportTime(1));
                }

                // Process the left nodes first to avoid the LTimes from incrementing
                // in the wrong order. Note that we don't have the actual Lamport time
                // for the leave message, so we go one past the join time, since the
                // leave must have been accepted after that to get onto the left members
                // list. If we didn't do this then the message would not get processed.
                for node in &pp.left_members {
                  if let Some(&ltime) = pp.status_ltimes.get(node) {
                    this
                      .handle_node_leave_intent(&LeaveMessage {
                        ltime,
                        node: node.cheap_clone(),
                        prune: false,
                      })
                      .await;
                  } else {
                    tracing::error!(
                      "ruserf: {} is in left members, but cannot find the lamport time for it in status",
                      node
                    );
                  }
                }

                // Update any other LTimes
                for (node, ltime) in pp.status_ltimes {
                  // Skip the left nodes
                  if pp.left_members.contains(&node) {
                    continue;
                  }

                  // Create an artificial join message
                  this
                    .handle_node_join_intent(&JoinMessage { ltime, node })
                    .await;
                }

                // If we are doing a join, and eventJoinIgnore is set
                // then we set the eventMinTime to the EventLTime. This
                // prevents any of the incoming events from being processed
                let event_join_ignore = this.inner.event_join_ignore.load(Ordering::Acquire);
                if is_join && event_join_ignore {
                  let mut ec = this.inner.event_core.write().await;
                  if pp.event_ltime > ec.min_time {
                    ec.min_time = pp.event_ltime;
                  }
                }

                // Process all the events
                for events in pp.events {
                  match events {
                    Some(events) => {
                      for e in events.events {
                        this
                          .handle_user_event(UserEventMessage {
                            ltime: events.ltime,
                            name: e.name,
                            payload: e.payload,
                            cc: false,
                          })
                          .await;
                      }
                    }
                    None => continue,
                  }
                }
              }
              msg => {
                tracing::error!("ruserf: remote state has bad type {}", msg.as_str());
              }
            }
          }
        }
      }
      ty => {
        tracing::error!("ruserf: remote state has bad type {}", ty.as_str());
      }
    }
  }
}

impl<D, T> EventDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  type Id = T::Id;
  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;

  async fn notify_join(&self, node: Arc<NodeState<Self::Id, Self::Address>>) {
    self.this().handle_node_join(node).await;
  }

  async fn notify_leave(&self, node: Arc<NodeState<Self::Id, Self::Address>>) {
    self.this().handle_node_leave(node).await;
  }

  async fn notify_update(&self, node: Arc<NodeState<Self::Id, Self::Address>>) {
    self.this().handle_node_update(node).await;
  }
}

impl<D, T> AliveDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  type Id = T::Id;
  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;
  type Error = Error<T, D>;

  async fn notify_alive(
    &self,
    node: Arc<NodeState<Self::Id, Self::Address>>,
  ) -> Result<(), Self::Error> {
    if let Some(ref d) = self.delegate {
      let member = node_to_member(node)?;
      return d
        .notify_merge(TinyVec::from(member))
        .await
        .map_err(|e| Error::Delegate(DelegateError::merge(e)));
    }

    Ok(())
  }
}

impl<D, T> MemberlistMergeDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
  <<T::Runtime as RuntimeLite>::Sleep as Future>::Output: Send,
  <<T::Runtime as RuntimeLite>::Interval as Stream>::Item: Send,
{
  type Id = T::Id;
  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;
  type Error = Error<T, D>;

  async fn notify_merge(
    &self,
    peers: SmallVec<Arc<NodeState<Self::Id, Self::Address>>>,
  ) -> Result<(), Self::Error> {
    if let Some(ref d) = self.delegate {
      let peers = peers
        .into_iter()
        .map(node_to_member)
        .collect::<Result<TinyVec<_>, _>>()?;
      return d
        .notify_merge(peers)
        .await
        .map_err(|e| Error::Delegate(DelegateError::merge(e)));
    }
    Ok(())
  }
}

impl<D, T> ConflictDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  type Id = T::Id;

  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;

  async fn notify_conflict(
    &self,
    existing: Arc<NodeState<Self::Id, Self::Address>>,
    other: Arc<NodeState<Self::Id, Self::Address>>,
  ) {
    self.this().handle_node_conflict(existing, other).await;
  }
}

impl<D, T> PingDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  type Id = T::Id;

  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;

  async fn ack_payload(&self) -> Bytes {
    if let Some(c) = self.this().inner.coord_core.as_ref() {
      let mut buf = Vec::new();
      buf.put_u8(PING_VERSION);

      if let Err(e) =
        <D as TransformDelegate>::encode_coordinate(&c.client.get_coordinate(), &mut buf[1..])
      {
        tracing::error!(err=%e, "ruserf: failed to encode coordinate");
      }
      buf.into()
    } else {
      Bytes::new()
    }
  }

  async fn notify_ping_complete(
    &self,
    node: Arc<NodeState<Self::Id, Self::Address>>,
    rtt: std::time::Duration,
    payload: Bytes,
  ) {
    if payload.is_empty() {
      return;
    }

    let this = self.this();

    if let Some(ref c) = this.inner.coord_core {
      // Verify ping version in the header.
      if payload[0] != PING_VERSION {
        tracing::error!("ruserf: unsupported ping version: {}", payload[0]);
        return;
      }

      // Process the remainder of the message as a coordinate.
      let coord = match <D as TransformDelegate>::decode_coordinate(&payload[1..]) {
        Ok((readed, c)) => {
          tracing::trace!(read=%readed, coordinate=?c, "ruserf: decode coordinate successfully");
          c
        }
        Err(e) => {
          tracing::error!(err=%e, "ruserf: failed to decode coordinate from ping");
          return;
        }
      };

      // Apply the update.
      let before = c.client.get_coordinate();
      match c.client.update(node.id(), &before, rtt) {
        Ok(after) => {
          #[cfg(feature = "metrics")]
          {
            // Publish some metrics to give us an idea of how much we are
            // adjusting each time we update.
            let d = before.distance_to(&after).as_secs_f64() * 1.0e3;
            metrics::histogram!(
              "ruserf.coordinate.adjustment-ms",
              this.inner.opts.memberlist_options.metric_labels.iter()
            )
            .record(d);
          }

          // Cache the coordinate for the other node, and add our own
          // to the cache as well since it just got updated. This lets
          // users call GetCachedCoordinate with our node name, which is
          // more friendly.
          let mut cache = c.cache.write();
          cache.insert(node.id().cheap_clone(), coord);
          cache
            .entry(this.inner.memberlist.local_id().cheap_clone())
            .and_modify(|x| {
              *x = c.client.get_coordinate();
            })
            .or_insert_with(|| c.client.get_coordinate());
        }
        Err(e) => {
          #[cfg(feature = "metrics")]
          {
            metrics::counter!(
              "ruserf.coordinate.rejected",
              this.inner.opts.memberlist_options.metric_labels.iter()
            )
            .increment(1);
          }

          tracing::error!(err=%e, "ruserf: rejected coordinate from {}", node.id());
        }
      }
    }
  }

  #[inline]
  fn disable_promised_pings(&self, _id: &Self::Id) -> bool {
    false
  }
}

impl<D, T> MemberlistDelegate for SerfDelegate<T, D>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  type Id = T::Id;

  type Address = <T::Resolver as AddressResolver>::ResolvedAddress;
}

fn node_to_member<T, D>(
  node: Arc<NodeState<T::Id, <T::Resolver as AddressResolver>::ResolvedAddress>>,
) -> Result<Member<T::Id, <T::Resolver as AddressResolver>::ResolvedAddress>, Error<T, D>>
where
  D: Delegate<Id = T::Id, Address = <T::Resolver as AddressResolver>::ResolvedAddress>,
  T: Transport,
{
  let status = if node.state() == State::Left {
    MemberStatus::Left
  } else {
    MemberStatus::None
  };

  let meta = node.meta();
  if meta.len() > META_MAX_SIZE {
    return Err(Error::TagsTooLarge(meta.len()));
  }

  Ok(Member {
    node: node.node(),
    tags: <D as TransformDelegate>::decode_tags(node.meta())
      .map_err(Error::transform)
      .map(|(read, tags)| {
        tracing::trace!(read=%read, tags=?tags, "ruserf: decode tags successfully");
        tags
      })?,
    status: Atomic::new(status),
    protocol_version: ProtocolVersion::V0,
    delegate_version: DelegateVersion::V0,
  })
}