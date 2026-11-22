#![cfg(feature = "desktop")]

use mosaic_shell_tools::audio::{AudioState, parse_snapshot};

#[test]
fn recognizes_default_devices_and_channel_volume() {
    let state = parse_snapshot(
        r#"{"default_sink_name":"speakers","default_source_name":"mic"}"#,
        r#"[{"name":"speakers","description":"Speakers","mute":false,
             "volume":{"left":{"value":32768},"right":{"value":65536}}}]"#,
        r#"[{"name":"mic","description":"Microphone","mute":true,"volume":{}}]"#,
        "[]",
        "[]",
    )
    .unwrap();
    assert!(state.available);
    assert_eq!(state.default_sink, "speakers");
    assert_eq!(state.default_source, "mic");
    assert_eq!(state.sinks[0].volume, Some(100));
    assert!(state.sources[0].muted);
    let wire = toml::to_string(&state).unwrap();
    assert_eq!(toml::from_str::<AudioState>(&wire).unwrap(), state);
}

#[test]
fn rejects_malformed_server_response() {
    assert!(parse_snapshot("{}", "invalid", "[]", "[]", "[]").is_err());
    assert!(!AudioState::default().available);
}

#[test]
fn playback_streams_link_to_outputs_without_conflating_two_clients() {
    let state = parse_snapshot(
        r#"{"default_sink_name":"headphones"}"#,
        r#"[{"index":7,"name":"headphones","description":"Headphones","mute":false},
            {"index":19,"name":"speakers","description":"Speakers","mute":false}]"#,
        "[]",
        r#"[
            {"index":24,"sink":7,"client":"2","channel_map":"left,right","mute":false,
             "volume":{"left":{"value":16384},"right":{"value":32768}},
             "properties":{"application.name":"Player","media.name":"Music"}},
            {"index":27,"sink":19,"client":3,"mute":true,
             "properties":{"application.process.binary":"game"}},
            {"index":29,"sink":99,"mute":false,"properties":{}}
        ]"#,
        "[]",
    )
    .unwrap();
    assert_eq!(state.playback[0].index, 24);
    assert_eq!(state.playback[0].application, "Player");
    assert_eq!(state.playback[0].media, "Music");
    assert_eq!(state.playback[0].output, "headphones");
    assert_eq!(state.playback[0].volume, Some(50));
    assert_eq!(state.playback[1].index, 27);
    assert_eq!(state.playback[1].application, "game");
    assert_eq!(state.playback[1].output, "speakers");
    assert!(state.playback[1].muted);
    assert!(state.playback[2].output.is_empty());
    assert_eq!(
        toml::from_str::<AudioState>(&toml::to_string(&state).unwrap()).unwrap(),
        state
    );
}

#[test]
fn stereo_balance_respects_channel_order_and_unavailable_values() {
    let state = parse_snapshot(
        "{}",
        r#"[
            {"name":"left-first","mute":false,"channel_map":"front-left,front-right",
             "volume":{"front-left":{"value":49152},"front-right":{"value":24576}}},
            {"name":"right-first","mute":false,"channel_map":"front-right,front-left",
             "volume":{"front-left":{"value":16384},"front-right":{"value":32768}}},
            {"name":"mono","mute":false,"channel_map":"mono",
             "volume":{"mono":{"value":32768}}},
            {"name":"silent","mute":true,"channel_map":"front-left,front-right",
             "volume":{"front-left":{"value":0},"front-right":{"value":0}}}
        ]"#,
        "[]",
        "[]",
        "[]",
    )
    .unwrap();
    assert_eq!(state.sinks[0].balance, Some(50));
    assert_eq!(state.sinks[1].balance, Some(150));
    assert_eq!(state.sinks[2].balance, None);
    assert_eq!(state.sinks[3].balance, None);
    assert_eq!(
        toml::from_str::<AudioState>(&toml::to_string(&state).unwrap()).unwrap(),
        state
    );
}

#[test]
fn audio_hardware_profiles_and_ports_keep_availability_and_active_ids() {
    let state = parse_snapshot(
        "{}",
        r#"[{"name":"speakers","mute":false,"active_port":"analog-output-lineout",
             "ports":[
                 {"name":"analog-output-headphones","description":"Headphones","availability":"not available","priority":9900},
                 {"name":"analog-output-lineout","description":"Line Out","availability":"availability unknown","priority":10000}
             ]}]"#,
        r#"[{"name":"mic","mute":false,"active_port":null,
             "ports":[{"name":"analog-input-mic","description":"Mic","availability":"available"}]}]"#,
        "[]",
        r#"[{"name":"alsa_card.test","properties":{"device.description":"Test card"},
             "active_profile":"output:analog-stereo+input:analog-stereo",
             "profiles":{"off":{"description":"Off","priority":0,"available":true},
                         "output:analog-stereo+input:analog-stereo":{"description":"Duplex","priority":6000,"available":true},
                         "output:hdmi-stereo":{"description":"HDMI","priority":100,"available":false}}}]"#,
    ).unwrap();
    let card = &state.cards[0];
    assert_eq!(card.description, "Test card");
    assert_eq!(card.active_profile, card.profiles[0].name);
    assert_eq!(card.profiles[0].description, "Duplex");
    assert!(!card.profiles[1].available);
    assert_eq!(state.sinks[0].active_port, "analog-output-lineout");
    assert!(state.sinks[0].ports[0].available);
    assert!(!state.sinks[0].ports[1].available);
    assert!(state.sources[0].active_port.is_empty());
    assert_eq!(
        toml::from_str::<AudioState>(&toml::to_string(&state).unwrap()).unwrap(),
        state
    );
}
