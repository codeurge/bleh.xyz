//! Bring your own feed (ADR 0014): merge a feed the user licenses
//! themselves with bleh.xyz data, on the user's machine only.
//!
//! The user's records are processed by this client and nowhere else. They
//! are never sent to a bleh.xyz service, never signed, and never written
//! into an archive layout, a manifest or a publishing path. Every output
//! built from them is tagged `user_supplied`. The project ships these
//! connectors (code that reads documented open formats and APIs with the
//! user's own credentials), never data.
//!
//! | Module | Contents |
//! |---|---|
//! | [`boundary`] | [`UserSupplied`], [`LocalPath`], [`LoopbackAddr`]: the boundary, enforced by types |
//! | [`mapping`] | The user-written mapping file (their symbol to a bleh.xyz market, synthetic month or exposure) |
//! | [`feed`] | [`UserQuote`] and the CSV / JSON-lines connector |
//! | [`dbn_feed`] | DBN files and (feature `databento-live`) the vendor's live client |
//! | [`bleh`] | Gateway events to bleh.xyz price observations |
//! | [`align`] | As-of alignment on a time grid |
//! | [`stats`] | Basis, tracking error, lead/lag |
//! | [`sink`] | JSON lines and the loopback websocket |
//! | [`run`] | The `mesh-client byo merge` driver |
//!
//! Operations: `docs/operations/byo-feed.md`.

pub mod align;
pub mod bleh;
pub mod boundary;
#[cfg(feature = "dbn-files")]
pub mod dbn_feed;
pub mod feed;
pub mod mapping;
pub mod run;
pub mod sink;
pub mod stats;

pub use boundary::{BoundaryError, LocalPath, LoopbackAddr, USER_SUPPLIED, UserSupplied};
pub use feed::UserQuote;
pub use mapping::Mapping;

#[cfg(test)]
mod tests {
    /// Every source file of this module, for the boundary scan.
    const SOURCES: &[(&str, &str)] = &[
        ("mod.rs", include_str!("mod.rs")),
        ("align.rs", include_str!("align.rs")),
        ("bleh.rs", include_str!("bleh.rs")),
        ("boundary.rs", include_str!("boundary.rs")),
        ("dbn_feed.rs", include_str!("dbn_feed.rs")),
        ("feed.rs", include_str!("feed.rs")),
        ("mapping.rs", include_str!("mapping.rs")),
        ("run.rs", include_str!("run.rs")),
        ("sink.rs", include_str!("sink.rs")),
        ("stats.rs", include_str!("stats.rs")),
    ];

    /// The module has no path to signing, Merkle commitments, manifests,
    /// key files, the archive or the bus, and never sends on the gateway
    /// connection. (The scan skips this test module itself.)
    #[test]
    fn no_path_to_signing_publishing_or_the_archive() {
        let forbidden = [
            "mesh_types::signing",
            "mesh_types::merkle",
            "mesh_types::manifest",
            "mesh_types::hash",
            "mesh_types::canonical",
            "crate::keys",
            "crate::verify",
            "archive::",
            "bus::",
            "async_nats",
            "object_store",
            ".send(ClientMessage",
            "client.send(",
            "ClientMessage::",
        ];
        for (name, src) in SOURCES {
            let body = match src.find("#[cfg(test)]\nmod tests") {
                Some(i) if *name == "mod.rs" => &src[..i],
                _ => src,
            };
            // Doc comments may name these to explain the rule; code may not.
            let code: String = body
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !(t.starts_with("//") || t.starts_with("///"))
                })
                .collect::<Vec<_>>()
                .join("\n");
            for f in forbidden {
                assert!(!code.contains(f), "byo/{name} references {f}");
            }
        }
    }

    /// The scan covers every file in the module directory.
    #[test]
    fn the_scan_lists_every_source_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/byo");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rs"))
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = SOURCES.iter().map(|(n, _)| (*n).to_string()).collect();
        listed.sort();
        assert_eq!(on_disk, listed);
    }

    /// Public user-data types implement no serde traits. (A type with
    /// `Serialize` could be fed to the canonical encoder.)
    #[test]
    fn user_data_types_are_not_serializable() {
        let types = [
            "UserSupplied",
            "UserQuote",
            "UserPoint",
            "Sample",
            "TrackingReport",
        ];
        let mut found = 0;
        for (name, src) in SOURCES {
            for ty in types {
                let decl = format!("pub struct {ty}");
                let at = src.match_indices(&decl).map(|(i, _)| i).find(|i| {
                    src[i + decl.len()..]
                        .chars()
                        .next()
                        .is_some_and(|c| !c.is_alphanumeric() && c != '_')
                });
                if let Some(i) = at {
                    found += 1;
                    let head = &src[..i];
                    let attrs: Vec<&str> = head
                        .rsplit("\n\n")
                        .next()
                        .unwrap_or("")
                        .lines()
                        .map(str::trim_start)
                        .filter(|l| l.starts_with("#["))
                        .collect();
                    assert!(
                        attrs
                            .iter()
                            .all(|a| !a.contains("Serialize") && !a.contains("serde")),
                        "byo/{name}: {ty} derives a serde trait"
                    );
                }
            }
        }
        assert_eq!(found, types.len(), "every listed type was found");
    }
}
