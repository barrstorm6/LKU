# Validation — 23 October 2026

## Environment

- Windows 11 Home, version 10.0.26200, x86-64.
- AMD Ryzen 5 5600H; AMD integrated graphics and NVIDIA RTX 3050 Ti Laptop GPU.
- Rust 1.99.1, `stable-x86_64-pc-windows-gnu`, existing MinGW-w64 GCC.
- Native wgpu 30 DirectX 12 backend; low-power adapter preference and
  `MemoryHints::MemoryUsage`. No browser or external GUI runtime processes are involved.
- Source dependency versions are pinned in Cargo.lock. A source-unchanged vendored
  serde-saphyr manifest patch resolves the egui/kube smallvec version conflict;
  see vendor/README.md.

## Executed checks

| Check                                                     | Result                                                               |
|-----------------------------------------------------------|----------------------------------------------------------------------|
| `cargo fmt --check`                                       | Passed                                                               |
| `cargo check`                                             | Passed                                                               |
| `cargo clippy -- -D warnings`                             | Passed                                                               |
| `cargo clippy --all-targets -- -D warnings`               | Passed                                                               |
| `cargo test`                                              | 10 passed; 3 explicitly ignored                                      |
| `cargo build --release`                                   | Passed; portable Windows .exe generated                              |
| `cargo test --release benchmark -- --ignored --nocapture` | Both benchmark tests passed                                          |
| PE import inspection                                      | Only Windows system DLLs; no MinGW runtime DLL dependency            |
| Release process smoke test                                | Process stayed alive, native window handle observed                  |
| Disposable real Kubernetes integration                    | Not run: Docker engine unavailable; no disposable cluster configured |
| Native visual/file-picker automation                      | Not completed: Computer Use app approval timed out                   |

Commands used the explicit GNU toolchain override. The default test command
ignores two release-only benchmarks and one opt-in real-cluster test. The
benchmarks were then executed separately. Tests include real kube-rs HTTP requests
against isolated test-only API fixtures for namespace-list 403, Pod list/watch
deletion, and container log streaming/cancellation with Cyrillic text.

Initial dependency resolution failed because egui requires smallvec >=1.16 and
serde-saphyr 0.0.29 declared <1.16. Widening only that bound resolved it; parsing
tests passed. Initial GUI compilation exposed egui 0.37 API changes, which were
fixed. A test-only Tokio IO feature omission was fixed. An initially racy
cancellation test was corrected to await worker termination; it now passes.
Sandboxed formatter/compiler checks were denied access and were rerun with the
approved workspace build permissions. No failed check is counted as passing.

## Measurements

Process measurements used a release build, no configured cluster or logs, a
5-second settling period, and a 10-second CPU sample. Windows process working set
includes mapped driver/runtime memory; private bytes are committed process memory.
The launcher requested a hidden start, so this is not a verified foreground
visual presentation benchmark. These are individual samples, not distributions.

| Metric                                       |                                                    Measured |
|----------------------------------------------|------------------------------------------------------------:|
| Delivered executable size                    |                                12,738,560 bytes (12.15 MiB) |
| Working set                                  |                              152,129,536 bytes (145.08 MiB) |
| Private bytes                                |                              209,465,344 bytes (199.76 MiB) |
| Idle CPU                                     | 0.624% of one logical CPU during the final 10-second sample |
| Launch to observed native main-window handle |                                                      429 ms |

The initial default graphics setup measured 247,115,776 bytes (235.67 MiB) working
set. Low-power adapter selection and memory-saving allocation hints reduced this
to 145–152 MiB across two subsequent samples (idle CPU 0.156–0.624% of one logical
CPU). **The under-80-MB RAM target was not met.** The binary-size target
was met. A main-window handle is only a startup proxy: first visible/presented
frame under one second remains unverified. Connected-state memory, long-run idle
CPU, GPU memory, and end-to-end interaction p95 remain unmeasured.

Release fixtures (200 timed samples after 20 UI warm-up frames):

| Simulated resources |         Search p95 | Filter + virtualized UI frame p95 |
|---------------------|-------------------:|----------------------------------:|
| 1,000 Pods          |  58.7 microseconds |                133.2 microseconds |
| 10,000 Pods         | 907.4 microseconds |                         1.1144 ms |

Appending 1,000 log lines per frame with a 5,000-line retained buffer and virtualized
viewer measured UI p95 232.2 microseconds. This synthetic benchmark exercises the
production buffer/filter/UI code directly. It does not measure network ingestion,
GPU execution, screen presentation, clipboard, or real input latency. Fixtures
exist only in `#[cfg(test)]`; production never substitutes simulated Pods.

## Remaining acceptance work

Run the UI against a reachable disposable cluster and verify native file selection,
context choice, namespace switching/RBAC, Pod updates, details/deletion, multiple
containers, logs across restart/VPN loss, and persisted restoration after normal
closure. Credentials and kubeconfig contents were not inspected or uploaded during
this build. The user's existing kubeconfig was not connected automatically.

The delivery is a compiled native implementation with passing automated fixtures,
not a claim that the full real-cluster definition of done was exercised here.

Delivered `dist/Kube.exe` SHA-256:
`DA8E5BC0D2D5F85948BA4146D5DC3D8AAA9D708C08A9FBCB8F6E175E5FBF5E67`.
