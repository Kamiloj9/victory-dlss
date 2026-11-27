#![cfg(windows)]

use victory_gpu::shader_cache;
use victory_shader::emit;

// A separate test binary keeps the process-wide translator profile and
// disk cache independent of tests that create a live D3D12 device.
#[test]
fn offline_cache_lookup_preserves_requested_adapter_facts() {
    let dir = tempfile::tempdir().unwrap();
    shader_cache::use_shader_cache(dir.path().into());
    emit::set_host_max_wave(64);
    emit::set_view_heap_limit(123_455);
    let profile = emit::host_settings();

    // This deliberately invalid container forces a cache miss and translation.
    assert!(shader_cache::warm(b"invalid shader container").is_err());
    assert_eq!(emit::host_settings(), profile);
}
