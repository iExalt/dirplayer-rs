use vm_rust::browser_e2e_test;

/// Exercises the browser WebSocket executor through the real owner-bound
/// prepare/execute/install path. The Playwright runner starts the loopback
/// Bun server and publishes its URL before the wasm test module loads.
browser_e2e_test!(test_multiuser_socket_lifecycle, |player| async move {
    player.test_multiuser_socket_lifecycle().await
});
