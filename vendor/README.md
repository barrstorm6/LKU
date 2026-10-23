serde-saphyr 0.0.29 is vendored from its crates.io release (MIT OR Apache-2.0).
Its source is unchanged. Cargo.toml widens the optional smallvec version bound
from `>= 1.0.0, < 1.16.0` to `>= 1.0.0, < 1.17.0` to resolve the conflict
between kube 4.2.0 and egui 0.37.0 (which requires smallvec 1.16).
Only runtime sources, manifest, README, and licenses are included.
Remove this patch when upstream kube uses a compatible YAML dependency.
