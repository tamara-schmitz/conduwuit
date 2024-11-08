use std::sync::Arc;

use conduit::{utils, warn, Error, Result};
use database::Map;
use ruma::{events::StateEventType, EventId, RoomId};

use crate::{globals, Dep};

pub(super) struct Data {
	eventid_shorteventid: Arc<Map>,
	shorteventid_eventid: Arc<Map>,
	statekey_shortstatekey: Arc<Map>,
	shortstatekey_statekey: Arc<Map>,
	roomid_shortroomid: Arc<Map>,
	statehash_shortstatehash: Arc<Map>,
	services: Services,
}

struct Services {
	globals: Dep<globals::Service>,
}

impl Data {
	pub(super) fn new(args: &crate::Args<'_>) -> Self {
		let db = &args.db;
		Self {
			eventid_shorteventid: db["eventid_shorteventid"].clone(),
			shorteventid_eventid: db["shorteventid_eventid"].clone(),
			statekey_shortstatekey: db["statekey_shortstatekey"].clone(),
			shortstatekey_statekey: db["shortstatekey_statekey"].clone(),
			roomid_shortroomid: db["roomid_shortroomid"].clone(),
			statehash_shortstatehash: db["statehash_shortstatehash"].clone(),
			services: Services {
				globals: args.depend::<globals::Service>("globals"),
			},
		}
	}

	pub(super) fn get_or_create_shorteventid(&self, event_id: &EventId) -> Result<u64> {
		let short = if let Some(shorteventid) = self.eventid_shorteventid.get(event_id.as_bytes())? {
			utils::u64_from_bytes(&shorteventid).map_err(|_| Error::bad_database("Invalid shorteventid in db."))?
		} else {
			let shorteventid = self.services.globals.next_count()?;
			self.eventid_shorteventid
				.insert(event_id.as_bytes(), &shorteventid.to_be_bytes())?;
			self.shorteventid_eventid
				.insert(&shorteventid.to_be_bytes(), event_id.as_bytes())?;
			shorteventid
		};

		Ok(short)
	}

	pub(super) fn multi_get_or_create_shorteventid(&self, event_ids: &[&EventId]) -> Result<Vec<u64>> {
		let mut ret: Vec<u64> = Vec::with_capacity(event_ids.len());
		let keys = event_ids
			.iter()
			.map(|id| id.as_bytes())
			.collect::<Vec<&[u8]>>();
		for (i, short) in self
			.eventid_shorteventid
			.multi_get(&keys)?
			.iter()
			.enumerate()
		{
			#[allow(clippy::single_match_else)]
			match short {
				Some(short) => ret.push(
					utils::u64_from_bytes(short).map_err(|_| Error::bad_database("Invalid shorteventid in db."))?,
				),
				None => {
					let short = self.services.globals.next_count()?;
					self.eventid_shorteventid
						.insert(keys[i], &short.to_be_bytes())?;
					self.shorteventid_eventid
						.insert(&short.to_be_bytes(), keys[i])?;

					debug_assert!(ret.len() == i, "position of result must match input");
					ret.push(short);
				},
			}
		}

		Ok(ret)
	}

	pub(super) fn get_shortstatekey(&self, event_type: &StateEventType, state_key: &str) -> Result<Option<u64>> {
		let mut statekey_vec = event_type.to_string().as_bytes().to_vec();
		statekey_vec.push(0xFF);
		statekey_vec.extend_from_slice(state_key.as_bytes());

		let short = self
			.statekey_shortstatekey
			.get(&statekey_vec)?
			.map(|shortstatekey| {
				utils::u64_from_bytes(&shortstatekey).map_err(|_| Error::bad_database("Invalid shortstatekey in db."))
			})
			.transpose()?;

		Ok(short)
	}

	pub(super) fn get_or_create_shortstatekey(&self, event_type: &StateEventType, state_key: &str) -> Result<u64> {
		let mut statekey_vec = event_type.to_string().as_bytes().to_vec();
		statekey_vec.push(0xFF);
		statekey_vec.extend_from_slice(state_key.as_bytes());

		let short = if let Some(shortstatekey) = self.statekey_shortstatekey.get(&statekey_vec)? {
			utils::u64_from_bytes(&shortstatekey).map_err(|_| Error::bad_database("Invalid shortstatekey in db."))?
		} else {
			let shortstatekey = self.services.globals.next_count()?;
			self.statekey_shortstatekey
				.insert(&statekey_vec, &shortstatekey.to_be_bytes())?;
			self.shortstatekey_statekey
				.insert(&shortstatekey.to_be_bytes(), &statekey_vec)?;
			shortstatekey
		};

		Ok(short)
	}

	pub(super) fn get_eventid_from_short(&self, shorteventid: u64) -> Result<Arc<EventId>> {
		let bytes = self
			.shorteventid_eventid
			.get(&shorteventid.to_be_bytes())?
			.ok_or_else(|| Error::bad_database("Shorteventid does not exist"))?;

		let event_id = EventId::parse_arc(
			utils::string_from_bytes(&bytes)
				.map_err(|_| Error::bad_database("EventID in shorteventid_eventid is invalid unicode."))?,
		)
		.map_err(|_| Error::bad_database("EventId in shorteventid_eventid is invalid."))?;

		Ok(event_id)
	}

	pub(super) fn get_statekey_from_short(&self, shortstatekey: u64) -> Result<(StateEventType, String)> {
		let bytes = self
			.shortstatekey_statekey
			.get(&shortstatekey.to_be_bytes())?
			.ok_or_else(|| Error::bad_database("Shortstatekey does not exist"))?;

		let mut parts = bytes.splitn(2, |&b| b == 0xFF);
		let eventtype_bytes = parts.next().expect("split always returns one entry");
		let statekey_bytes = parts
			.next()
			.ok_or_else(|| Error::bad_database("Invalid statekey in shortstatekey_statekey."))?;

		let event_type = StateEventType::from(utils::string_from_bytes(eventtype_bytes).map_err(|e| {
			warn!("Event type in shortstatekey_statekey is invalid: {}", e);
			Error::bad_database("Event type in shortstatekey_statekey is invalid.")
		})?);

		let state_key = utils::string_from_bytes(statekey_bytes)
			.map_err(|_| Error::bad_database("Statekey in shortstatekey_statekey is invalid unicode."))?;

		let result = (event_type, state_key);

		Ok(result)
	}

	/// Returns (shortstatehash, already_existed)
	pub(super) fn get_or_create_shortstatehash(&self, state_hash: &[u8]) -> Result<(u64, bool)> {
		Ok(if let Some(shortstatehash) = self.statehash_shortstatehash.get(state_hash)? {
			(
				utils::u64_from_bytes(&shortstatehash)
					.map_err(|_| Error::bad_database("Invalid shortstatehash in db."))?,
				true,
			)
		} else {
			let shortstatehash = self.services.globals.next_count()?;
			self.statehash_shortstatehash
				.insert(state_hash, &shortstatehash.to_be_bytes())?;
			(shortstatehash, false)
		})
	}

	pub(super) fn get_shortroomid(&self, room_id: &RoomId) -> Result<Option<u64>> {
		self.roomid_shortroomid
			.get(room_id.as_bytes())?
			.map(|bytes| utils::u64_from_bytes(&bytes).map_err(|_| Error::bad_database("Invalid shortroomid in db.")))
			.transpose()
	}

	pub(super) fn get_or_create_shortroomid(&self, room_id: &RoomId) -> Result<u64> {
		Ok(if let Some(short) = self.roomid_shortroomid.get(room_id.as_bytes())? {
			utils::u64_from_bytes(&short).map_err(|_| Error::bad_database("Invalid shortroomid in db."))?
		} else {
			let short = self.services.globals.next_count()?;
			self.roomid_shortroomid
				.insert(room_id.as_bytes(), &short.to_be_bytes())?;
			short
		})
	}
}
