# Kube

A compact, read-only Kubernetes Pod viewer for Windows 11. Rust 2024,
egui/eframe 0.37, native wgpu rendering, kube-rs 4.2. No browser or local UI server.

## Run

Open `dist/Kube.exe`. The portable executable needs no Rust, Docker, kubectl,
or Node installation. A DirectX 12 capable graphics driver is required.
Credential programs referenced by your kubeconfig must be installed separately.

1. Click **+ Add cluster** and choose a kubeconfig. Use **All files** for an
   extensionless `config` file.
2. Select the context and click **Connect**. If the context uses an exec
   credential program, review the configuration source and explicitly trust it.
3. Choose a namespace. If namespace listing is forbidden, enter an accessible
   namespace and click **Go**.
4. Search Pods locally, click one for details, then click **Logs**.
5. Select a container, optionally show timestamps, or disable **Auto-scroll**.
   Search filters the retained log buffer. Select text within a line and press **Ctrl+C** to copy
   it.

The connection dropdown switches clusters, adds another configuration/context,
and removes saved connections with **×**. Removal never deletes kubeconfig files.
Hover a connection to see its source path and context. Errors provide **Retry**
and **Choose kubeconfig…** for replacing a missing configuration.

Settings restore the last connection and its namespace. On Windows they live at
`%APPDATA%\Kube\config\settings.json`; other systems use their conventional
per-user config directory. Settings contain paths and preferences only. Kube
never modifies kubeconfig files or persists credentials or Pod logs.

## Build

Install stable Rust 1.99 or newer. For the standard Windows MSVC target, install
Visual Studio Build Tools with **Desktop development with C++** and a Windows SDK.

```powershell
cargo build --release --locked
.\target\release\Kube.exe
```

The delivered build uses `stable-x86_64-pc-windows-gnu` and MinGW-w64; this does
not add a MinGW runtime requirement to the delivered executable. For GNU builds,
install that Rust toolchain and place MinGW-w64's `bin` directory on PATH:

```powershell
cargo +stable-x86_64-pc-windows-gnu build --release --locked
```

Release builds use size optimization, thin LTO, one codegen unit, stripped
symbols, and `panic=abort`. Recoverable network/configuration failures use Results.
Windows uses wgpu's DirectX backend, a low-power adapter preference, and
memory-saving allocation hints. UI rendering is event driven.

## Validate

```powershell
cargo fmt --check
cargo check --locked
cargo clippy -- -D warnings
cargo test --locked
cargo build --release --locked
cargo test --release benchmark -- --ignored --nocapture
```

Unit tests cover kubeconfig/context selection, TLS rejection and exec trust
detection, Pod health, incremental watcher state, filtering, bounded logs,
settings replacement/round-trip, cancellation, and stale events. An isolated
loopback API fixture tests namespace 403 fallback plus initial Pod listing and
watch deletion. It is compiled only in tests; production uses real Kubernetes APIs.

For an optional read-only smoke test against a disposable local Kubernetes cluster
with a ready Pod and readable logs:

```powershell
$env:KUBE_TEST_CONFIG = 'C:\path\to\disposable-cluster.yaml'
cargo test disposable_cluster_read_only_smoke -- --ignored
```

That fixture must use verified HTTPS and must not require an exec program.
See [VALIDATION.md](VALIDATION.md) for executed checks and measured results.

## Boundaries and limitations

- Namespace and Pod access respects RBAC. A namespace-list 403 is independent of
  Pod permissions; a Pod-list 403 is shown as **Access denied**.
- Green requires a successful Pod list/update and readiness conditions plus ready
  regular containers. Pending, completed, terminating, and unknown Pods are gray.
- Watchers relist when needed and use backoff. Switching connection/namespace
  cancels workers; generation IDs reject late events. Watch network reads can take
  up to 330 seconds to time out during a silent network failure; the UI stays usable.
- Logs retain at most 5,000 lines and 8 MiB, truncating individual lines at 16 KiB.
  Initial history is the last 500 lines. Streaming uses bounded batches/channels.
  Reconnect resumes from the last received timestamp when available; Kubernetes'
  inclusive timestamp semantics may repeat boundary entries. Logs already rotated
  away by Kubernetes cannot be recovered. Previous terminated-container logs and
  multiline text selection across virtual rows are outside this MVP.
- Exec trust is requested again on each connection, including restoration. Trust
  is not silently saved. Review the entire source configuration, including plugin
  arguments and environment, before trusting it. Legacy `auth-provider` configs
  are unsupported; use an exec credential plugin. Interactive terminal-only auth
  plugins may require authentication in an external terminal first.
- HTTP and configurations that disable TLS verification are rejected. Errors are
  sanitized; dependency diagnostics are excluded from the application's subscriber.
- The Windows executable is built and process-tested. Full visual/file-picker QA,
  real cluster connectivity, real logs, and end-to-end restoration still require
  verification on a reachable test cluster. Linux/macOS paths are supported in
  code, but those targets have not been built or tested.
- The measured RAM target is reported honestly in VALIDATION.md; it is not a
  guarantee. No mutations, dashboards, telemetry, terminal, or YAML editor exist.
