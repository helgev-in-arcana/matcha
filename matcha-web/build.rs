use std::path::{Path, PathBuf};

// Defines the `web` cfg alias used throughout the crate.
// The alias expansion is kept in sync with the `cfg(...)` expressions in
// this crate's `[target.'cfg(...)']` sections in Cargo.toml.
fn main() {
    cfg_aliases::cfg_aliases! {
        web: { all(target_arch = "wasm32", target_os = "unknown") },
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MATCHA_EMBEDDED_FONT");

    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        font::ensure_available();
    }
}

/// Obtaining the font that `matcha-web` embeds.
mod font {
    use super::*;

    const URL: &str = "https://raw.githubusercontent.com/notofonts/noto-cjk/\
                       Sans2.004/Sans/Variable/TTF/Subset/NotoSansJP-VF.ttf";
    const SHA256: &str = "f4b373b226668ee33a6e54b02823dcd2d1209f17159f777421ae8c2275160369";
    const DEST: &str = "src/assets/NotoSansJP-VF.ttf";

    pub fn ensure_available() {
        let dest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"))
            .join(DEST);

        if let Some(path) = std::env::var_os("MATCHA_EMBEDDED_FONT") {
            let path = PathBuf::from(path);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                panic!("MATCHA_EMBEDDED_FONT points at {}: {e}", path.display())
            });
            write(&dest, &bytes);
            println!("cargo:warning=embedding the font from {}", path.display());
            return;
        }

        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == EXPECTED_LEN) {
            return;
        }

        println!("cargo:warning=downloading the embedded font (~9.6 MB, once) from {URL}");
        let bytes = download().unwrap_or_else(|e| {
            panic!(
                "could not download the font the web build embeds: {e}\n\
                 \n\
                 Fetch it manually and the build will use it:\n\
                 \n    curl -Lo {DEST} {URL}\n\
                 \n\
                 Or point MATCHA_EMBEDDED_FONT at any .ttf/.otf you already have."
            )
        });

        let actual = sha256_hex(&bytes);
        assert_eq!(
            actual, SHA256,
            "the font downloaded from {URL} is not the pinned revision \
             (expected SHA-256 {SHA256}, got {actual}); refusing to embed it"
        );

        write(&dest, &bytes);
    }

    const EXPECTED_LEN: u64 = 9_590_732;

    fn write(dest: &Path, bytes: &[u8]) {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|e| panic!("could not create {}: {e}", dir.display()));
        }
        std::fs::write(dest, bytes)
            .unwrap_or_else(|e| panic!("could not write {}: {e}", dest.display()));
    }

    fn download() -> Result<Vec<u8>, String> {
        let out = std::env::var("OUT_DIR").expect("cargo sets this");
        let tmp = Path::new(&out).join("font-download.ttf");

        let status = std::process::Command::new("curl")
            .args(["--location", "--fail", "--silent", "--show-error", "--output"])
            .arg(&tmp)
            .arg(URL)
            .status()
            .map_err(|e| format!("could not run curl: {e}"))?;

        if !status.success() {
            return Err(format!("curl exited with {status}"));
        }
        std::fs::read(&tmp).map_err(|e| format!("could not read {}: {e}", tmp.display()))
    }

    fn sha256_hex(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2,
        ];
        let mut h: [u32; 8] = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];

        let mut message = data.to_vec();
        let bit_len = (data.len() as u64) * 8;
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&bit_len.to_be_bytes());

        for chunk in message.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in chunk.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16]
                    .wrapping_add(s0)
                    .wrapping_add(w[i - 7])
                    .wrapping_add(s1);
            }

            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh
                    .wrapping_add(s1)
                    .wrapping_add(ch)
                    .wrapping_add(K[i])
                    .wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);

                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }

            for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *slot = slot.wrapping_add(value);
            }
        }

        h.iter().map(|word| format!("{word:08x}")).collect()
    }
}
