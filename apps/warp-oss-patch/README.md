# warp-oss-patch — One-line override-gate bypass

This directory contains the minimal patch to Warp's open-source binary
that makes Phase α of Lumen possible. Without it, the OSS build silently
ignores `WARP_SERVER_ROOT_URL` / `WARP_WS_SERVER_URL` (release-channel
hardening at `crates/warp_core/src/channel/mod.rs:46`).

## Usage

`./apply-patch.sh`

Clones `warpdotdev/warp` into `upstream/`, applies our one-line patch,
builds `target/release/warp-oss`. Re-runnable — refreshes the clone,
reapplies the patch (or skips if already applied).

## System dependencies

On macOS, the build requires the **Metal Toolchain** Xcode optional
component (Warp compiles Metal shaders at build time). If you see
`error compiling metal shaders to .air; error: cannot execute tool 'metal'`,
install it via:

```
xcodebuild -downloadComponent MetalToolchain
```

On Linux, the build typically requires `libvulkan-dev`, `libwayland-dev`,
and standard X11 dev libs. On Windows, the Vulkan SDK is required.

## Running the patched binary against lifegw

```
WARP_SERVER_ROOT_URL=http://localhost:8443 \
WARP_WS_SERVER_URL=ws://localhost:8443/graphql/v2 \
./upstream/target/release/warp-oss
```

## The patch

`channel-oss-override.patch` moves `Channel::Oss` from the `false` arm
to the `true` arm of `Channel::allows_server_url_overrides`. That's it.

After Phase α validates the integration, Phase β replaces this directory
with a real soft fork at `apps/lumen/` carrying its own branding,
defaults, and additional patches (Firebase API key neutralization, etc.).

## License

The patched binary inherits Warp's AGPLv3. Our patch is also AGPLv3 for
consistency.
