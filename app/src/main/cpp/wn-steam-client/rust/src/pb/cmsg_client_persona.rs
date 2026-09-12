use crate::proto_wire::{Reader, WireType, Writer};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CMsgClientRequestFriendData {
    pub persona_state_requested: u32,
    pub friends: Vec<u64>,
}

impl CMsgClientRequestFriendData {
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut w = Writer::new(&mut out);
        w.uint32_field(1, self.persona_state_requested);
        for id in &self.friends {
            w.fixed64_field(2, *id);
        }
        out
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PersonaStateFriend {
    pub friendid: u64,
    pub persona_state: u32,
    pub game_played_app_id: u32,
    pub player_name: String,
    pub avatar_hash: Vec<u8>,
    pub game_name: String,
    pub gameid: u64,
    pub rich_presence: Vec<(String, String)>,
    // None = the field was absent (keep what we have); Some(0) = the server cleared it.
    pub game_lobby_id: Option<u64>,
    pub game_server_ip: Option<u32>,
    pub game_server_port: Option<u32>,
    pub has_persona_state: bool,
    pub has_game: bool,
}

impl PersonaStateFriend {
    pub fn deserialize(body: &[u8]) -> Option<Self> {
        let mut reader = Reader::new(body);
        let mut msg = Self::default();
        while !reader.eof() {
            let Some(tag) = reader.next_tag() else {
                return reader.ok().then_some(msg);
            };
            match (tag.field_number, tag.wire_type) {
                (1, WireType::Fixed64) => msg.friendid = reader.fixed64()?,
                (2, WireType::Varint) => {
                    msg.persona_state = reader.u32()?;
                    msg.has_persona_state = true;
                }
                (3, WireType::Varint) => {
                    msg.game_played_app_id = reader.u32()?;
                    msg.has_game = true;
                }
                (4, WireType::Varint) => msg.game_server_ip = Some(reader.u32()?),
                (5, WireType::Varint) => msg.game_server_port = Some(reader.u32()?),
                (15, WireType::LengthDelimited) => msg.player_name = reader.string()?,
                (31, WireType::LengthDelimited) => msg.avatar_hash = reader.bytes()?.to_vec(),
                (55, WireType::LengthDelimited) => msg.game_name = reader.string()?,
                (56, WireType::Fixed64) => msg.gameid = reader.fixed64()?,
                (71, WireType::LengthDelimited) => msg
                    .rich_presence
                    .push(parse_kv_submessage(reader.bytes()?)?),
                (73, WireType::Fixed64) => msg.game_lobby_id = Some(reader.fixed64()?),
                _ => {
                    if !reader.skip(tag.wire_type) {
                        return None;
                    }
                }
            }
        }
        Some(msg)
    }
}

fn parse_kv_submessage(body: &[u8]) -> Option<(String, String)> {
    let mut reader = Reader::new(body);
    let mut key = String::new();
    let mut value = String::new();
    while !reader.eof() {
        let Some(tag) = reader.next_tag() else {
            return reader.ok().then_some((key, value));
        };
        match tag.field_number {
            1 => key = reader.string()?,
            2 => value = reader.string()?,
            _ => {
                if !reader.skip(tag.wire_type) {
                    return None;
                }
            }
        }
    }
    Some((key, value))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CMsgClientPersonaState {
    pub status_flags: u32,
    pub friends: Vec<PersonaStateFriend>,
}

impl CMsgClientPersonaState {
    pub fn deserialize(body: &[u8]) -> Option<Self> {
        let mut reader = Reader::new(body);
        let mut msg = Self::default();
        while !reader.eof() {
            let Some(tag) = reader.next_tag() else {
                return reader.ok().then_some(msg);
            };
            match tag.field_number {
                1 => msg.status_flags = reader.u32()?,
                2 => msg
                    .friends
                    .push(PersonaStateFriend::deserialize(reader.bytes()?)?),
                _ => {
                    if !reader.skip(tag.wire_type) {
                        return None;
                    }
                }
            }
        }
        Some(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto_wire::Writer;

    #[test]
    fn parses_persona_state_friend_with_rich_presence() {
        let mut kv = Vec::new();
        {
            let mut w = Writer::new(&mut kv);
            w.string_field(1, "status");
            w.string_field(2, "Playing");
        }

        let mut friend = Vec::new();
        {
            let mut w = Writer::new(&mut friend);
            w.fixed64_field(1, 123);
            w.uint32_field(2, 1);
            w.uint32_field(3, 440);
            w.string_field(15, "Ada");
            w.submessage_field(71, &kv);
            w.bytes_field(31, &[1, 2, 3]);
            w.string_field(55, "Team Fortress 2");
            w.fixed64_field(56, 440);
        }

        let mut body = Vec::new();
        Writer::new(&mut body).submessage_field(2, &friend);

        let parsed = CMsgClientPersonaState::deserialize(&body).unwrap();
        let friend = &parsed.friends[0];
        assert_eq!(friend.friendid, 123);
        assert_eq!(friend.player_name, "Ada");
        assert_eq!(friend.rich_presence[0], ("status".into(), "Playing".into()));
        assert_eq!(friend.avatar_hash, [1, 2, 3]);
        assert_eq!(friend.game_name, "Team Fortress 2");
        assert!(friend.has_persona_state);
        assert!(friend.has_game);
    }

    #[test]
    fn stateful_push_with_steamid_source_is_skipped_not_read_as_rich_presence() {
        // Field 25 is steamid_source (a fixed64), not rich presence — reading it as rich
        // presence is what kept every friend's connect string empty.
        let mut friend = Vec::new();
        {
            let mut w = Writer::new(&mut friend);
            w.fixed64_field(1, 77);
            w.uint32_field(2, 1);
            w.fixed64_field(25, 90071996842377216);
            w.string_field(15, "Online Friend");
        }
        let mut body = Vec::new();
        Writer::new(&mut body).submessage_field(2, &friend);

        let parsed = CMsgClientPersonaState::deserialize(&body).unwrap();
        let friend = &parsed.friends[0];
        assert_eq!(friend.friendid, 77);
        assert_eq!(friend.persona_state, 1);
        assert!(friend.has_persona_state);
        assert_eq!(friend.player_name, "Online Friend");
        assert!(friend.rich_presence.is_empty());
    }

    #[test]
    fn parses_lobby_and_game_server_fields() {
        let mut friend = Vec::new();
        {
            let mut w = Writer::new(&mut friend);
            w.fixed64_field(1, 5);
            w.uint32_field(3, 3527290);
            w.uint32_field(4, 0x681E100F);
            w.uint32_field(5, 27015);
            w.fixed64_field(73, 109775241234567890);
        }
        let mut body = Vec::new();
        Writer::new(&mut body).submessage_field(2, &friend);

        let parsed = CMsgClientPersonaState::deserialize(&body).unwrap();
        let friend = &parsed.friends[0];
        assert_eq!(friend.game_played_app_id, 3527290);
        assert_eq!(friend.game_server_ip, Some(0x681E100F));
        assert_eq!(friend.game_server_port, Some(27015));
        assert_eq!(friend.game_lobby_id, Some(109775241234567890));
    }

    #[test]
    fn explicit_zero_lobby_id_is_present_not_absent() {
        // Leaving a lobby arrives as an explicit 0, which must clear the stored id. The Writer
        // helpers omit zero-valued fields, so emit the tag and payload directly.
        let mut friend = Vec::new();
        {
            let mut w = Writer::new(&mut friend);
            w.fixed64_field(1, 5);
            w.tag(73, WireType::Fixed64);
            w.raw_bytes(&0u64.to_le_bytes());
            w.uint32_field_force(4, 0);
        }
        let mut body = Vec::new();
        Writer::new(&mut body).submessage_field(2, &friend);

        let parsed = CMsgClientPersonaState::deserialize(&body).unwrap();
        let friend = &parsed.friends[0];
        assert_eq!(friend.game_lobby_id, Some(0));
        assert_eq!(friend.game_server_ip, Some(0));
    }

    #[test]
    fn absent_lobby_id_stays_none() {
        let mut friend = Vec::new();
        {
            let mut w = Writer::new(&mut friend);
            w.fixed64_field(1, 5);
            w.uint32_field(2, 1);
        }
        let mut body = Vec::new();
        Writer::new(&mut body).submessage_field(2, &friend);

        let parsed = CMsgClientPersonaState::deserialize(&body).unwrap();
        let friend = &parsed.friends[0];
        assert_eq!(friend.game_lobby_id, None);
        assert_eq!(friend.game_server_ip, None);
        assert_eq!(friend.game_server_port, None);
    }
}
