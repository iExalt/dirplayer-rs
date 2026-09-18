use vm_rust::browser_e2e_test;

browser_e2e_test!(test_browser_handle_nested_input, |player| async move {
    player.test_browser_handle_nested_input().await
});

browser_e2e_test!(test_browser_handle_public_play, |player| async move {
    player.test_browser_handle_public_play().await
});

browser_e2e_test!(test_browser_handle_flash_scripted_access_owner_capabilities, |player| async move {
    player.test_flash_scripted_access_owner_capabilities().await
});

browser_e2e_test!(test_flash_owned_evaluator_binding, |player| async move {
    player.test_flash_owned_evaluator_binding().await
});

browser_e2e_test!(test_flash_initial_access_before_reservation, |player| async move {
    player.test_flash_initial_access_before_reservation().await
});

browser_e2e_test!(test_flash_sprite_variable_owned_fixture, |player| async move {
    player.test_flash_sprite_variable_owned_fixture().await
});

browser_e2e_test!(test_nested_flash_owned_fixture, |player| async move {
    player.test_nested_flash_owned_fixture().await
});

browser_e2e_test!(test_flash_lingo_callback_owned_fixture, |player| async move {
    player.test_flash_lingo_callback_owned_fixture().await
});

browser_e2e_test!(test_flash_mouse_dispatch_decoder, |player| async move {
    player.test_flash_mouse_dispatch_decoder().await
});

browser_e2e_test!(test_owned_mouse_pointer, |player| async move {
    player.test_owned_mouse_pointer().await
});

browser_e2e_test!(test_owned_mouse_reset_reentry, |player| async move {
    player.test_owned_mouse_reset_reentry().await
});

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_nested_flash_fixture_uses_production_director_parser() {
    for (name, bytes) in [
        (
            "nested_flash_a.dcr",
            include_bytes!("../../fixtures/nested_flash_a.dcr").to_vec(),
        ),
        (
            "nested_flash_b.dcr",
            include_bytes!("../../fixtures/nested_flash_b.dcr").to_vec(),
        ),
        (
            "nested_flash_bad.dcr",
            include_bytes!("../../fixtures/nested_flash_bad.dcr").to_vec(),
        ),
    ] {
        let parsed = vm_rust::director::file::read_director_file_bytes(
            &bytes,
            name,
            "https://fixture.invalid/",
        )
            .unwrap_or_else(|error| panic!("{name} did not parse through read_director_file_bytes: {error}"));
        assert_eq!(parsed.base_path.as_str(), "https://fixture.invalid/", "{name} base URL");
        assert_eq!(parsed.version, 500, "{name} should use the authored D5 human version");
        assert_eq!(parsed.cast_entries.len(), 1, "{name} should expose one cast entry");
        assert!(parsed.key_table.is_some(), "{name} should expose its KEY* table");
        let score = parsed.score.as_ref().expect("{name} should expose its VWSC score");
        let d5_channel_one = score
            .frame_data
            .frame_channel_data
            .iter()
            .find(|(frame, channel, _)| *frame == 0 && *channel == 6)
            .map(|(_, _, data)| (data.cast_lib, data.cast_member));
        assert_eq!(d5_channel_one, Some((1, 1)), "{name} D5 channel 1 frame data");
    }
}
