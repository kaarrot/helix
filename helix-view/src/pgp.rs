//! Passphrase-encrypted (`gpg -c`) files.
//!
//! Only symmetric messages are handled: a passphrase packet (SKESK) and
//! integrity-protected data (SEIPD). A file encrypted to a public key is
//! refused rather than opened, as saving it could only drop its recipients.
//!
//! While an encrypted document is open its plaintext stays inside the editor:
//! no language servers, formatters, diff base or word index see it. Once one
//! is opened, freed memory is zeroed by `helix_stdx::alloc`, which wipes the
//! copies of the plaintext and the passphrases as they go.
//!
//! All of it needs the `pgp` feature. Without it no path is taken for an
//! encrypted file, and the editor never gets past the checks below.

use std::fmt;
use std::path::{Path, PathBuf};

#[cfg(feature = "pgp")]
use {
    anyhow::{anyhow, ensure},
    pgp::composed::{ArmorOptions, Edata, Esk, Message, MessageBuilder},
    pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm},
    pgp::types::{Password, StringToKey},
    std::io::Read,
};

/// Extensions of files that may hold an encrypted message.
const EXTENSIONS: [&str; 3] = ["gpg", "pgp", "asc"];

/// The extensions a new file is encrypted for. A new `.asc` is more likely a
/// key or a signature written by hand, so it stays plain.
const NEW_FILE_EXTENSIONS: [&str; 2] = ["gpg", "pgp"];

/// The coded S2K iteration count: 39,845,888 bytes hashed, what `gpg -c`
/// calibrated to on the phone this was written on.
#[cfg(feature = "pgp")]
const S2K_COUNT: u8 = 243;

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| extensions.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Whether the file at `path` may hold an encrypted message. Never without
/// the `pgp` feature, which keeps every encrypted-file path out of reach.
pub fn is_pgp_path(path: &Path) -> bool {
    cfg!(feature = "pgp") && has_extension(path, &EXTENSIONS)
}

/// Whether a new file at `path` is encrypted when saved.
pub fn is_new_file_path(path: &Path) -> bool {
    cfg!(feature = "pgp") && has_extension(path, &NEW_FILE_EXTENSIONS)
}

/// `path` without its encryption extension, so `notes.md.gpg` is detected
/// as Markdown.
pub fn inner_path(path: &Path) -> PathBuf {
    if is_pgp_path(path) {
        path.with_extension("")
    } else {
        path.to_path_buf()
    }
}

/// A passphrase. The allocator wipes it when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Passphrase(Vec<u8>);

impl Passphrase {
    pub fn new(passphrase: &str) -> Self {
        Self(passphrase.as_bytes().to_vec())
    }

    #[cfg(feature = "pgp")]
    fn password(&self) -> Password {
        Password::from(&self.0[..])
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(***)")
    }
}

/// The passphrases that opened a file this session, newest first. Opening a
/// file tries them before asking.
#[derive(Debug, Default)]
pub struct Passphrases(Vec<Passphrase>);

impl Passphrases {
    pub fn remember(&mut self, passphrase: Passphrase) {
        self.0.retain(|known| *known != passphrase);
        self.0.insert(0, passphrase);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Passphrase> {
        self.0.iter()
    }

    pub fn forget(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What a file's bytes hold.
#[derive(Debug, PartialEq, Eq)]
pub enum Contents {
    /// Not an encrypted message: open the file as text.
    Plain,
    /// A message encrypted with a passphrase only.
    Encrypted { armored: bool },
    /// An encrypted message this module can't edit.
    Unsupported(&'static str),
}

#[cfg(feature = "pgp")]
pub fn inspect(bytes: &[u8]) -> Contents {
    let Ok((message, headers)) = Message::from_reader(bytes) else {
        return Contents::Plain;
    };
    let Message::Encrypted { esk, edata, .. } = &message else {
        return Contents::Plain;
    };
    if esk
        .iter()
        .any(|esk| matches!(esk, Esk::PublicKeyEncryptedSessionKey(_)))
    {
        return Contents::Unsupported(
            "encrypted to a public key; only passphrase (gpg -c) files are supported",
        );
    }
    if esk.is_empty() || matches!(edata, Edata::SymEncryptedData { .. }) {
        return Contents::Unsupported("uses legacy encryption without an integrity check");
    }
    Contents::Encrypted {
        armored: headers.is_some(),
    }
}

/// The passphrase didn't decrypt the file.
#[derive(Debug, thiserror::Error)]
#[error("wrong passphrase, or the file is damaged")]
pub struct BadPassphrase;

/// Decrypts a message [`inspect`] found to be [`Contents::Encrypted`].
///
/// rPGP checks the integrity code before it releases any plaintext, so a wrong
/// passphrase fails here rather than giving garbage.
#[cfg(feature = "pgp")]
pub fn decrypt(bytes: &[u8], passphrase: &Passphrase) -> Result<Vec<u8>, BadPassphrase> {
    helix_stdx::alloc::enable();
    let (message, _) = Message::from_reader(bytes).map_err(|_| BadPassphrase)?;
    let mut message = message
        .decrypt_with_password(&passphrase.password())
        .map_err(|_| BadPassphrase)?;
    while message.is_compressed() {
        message = message.decompress().map_err(|_| BadPassphrase)?;
    }
    let mut plaintext = Vec::new();
    message
        .read_to_end(&mut plaintext)
        .map_err(|_| BadPassphrase)?;
    Ok(plaintext)
}

/// Encrypts `plaintext` the way `gpg -c` does, minus compression. The output
/// is decrypted again and compared, so a file that can't be read back is
/// never returned.
#[cfg(feature = "pgp")]
pub fn encrypt(
    plaintext: &[u8],
    passphrase: &Passphrase,
    armored: bool,
    file_name: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut rng = rand::thread_rng();
    let mut builder = MessageBuilder::from_bytes(file_name.to_owned(), plaintext.to_vec())
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder.encrypt_with_password(
        StringToKey::new_iterated(&mut rng, HashAlgorithm::Sha256, S2K_COUNT),
        &passphrase.password(),
    )?;
    let ciphertext = if armored {
        builder
            .to_armored_string(&mut rng, ArmorOptions::default())?
            .into_bytes()
    } else {
        builder.to_vec(&mut rng)?
    };

    let roundtrip = decrypt(&ciphertext, passphrase)
        .map_err(|_| anyhow!("the encrypted file could not be decrypted again"))?;
    ensure!(
        roundtrip[..] == plaintext[..],
        "the encrypted file did not decrypt to the same text"
    );
    Ok(ciphertext)
}

// Without the `pgp` feature nothing is encrypted: `is_pgp_path` keeps these
// from being reached.

#[cfg(not(feature = "pgp"))]
pub fn inspect(_bytes: &[u8]) -> Contents {
    Contents::Plain
}

#[cfg(not(feature = "pgp"))]
pub fn decrypt(_bytes: &[u8], _passphrase: &Passphrase) -> Result<Vec<u8>, BadPassphrase> {
    Err(BadPassphrase)
}

#[cfg(not(feature = "pgp"))]
pub fn encrypt(
    _plaintext: &[u8],
    _passphrase: &Passphrase,
    _armored: bool,
    _file_name: &str,
) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("Helix was built without the `pgp` feature")
}

/// An encrypted document's state.
#[derive(Debug)]
pub struct PgpState {
    /// Whether the file is ASCII armored, which is kept on save.
    pub armored: bool,
    /// The file's passphrase. `None` while the document is locked.
    pub passphrase: Option<Passphrase>,
    /// The file's bytes while the document is locked. `None` for a new file.
    pub(crate) ciphertext: Option<Vec<u8>>,
}

impl PgpState {
    /// Starts wiping freed memory: the passphrase prompt and the plaintext
    /// follow.
    pub(crate) fn locked(armored: bool, ciphertext: Vec<u8>) -> Self {
        #[cfg(feature = "pgp")]
        helix_stdx::alloc::enable();
        Self {
            armored,
            passphrase: None,
            ciphertext: Some(ciphertext),
        }
    }

    pub(crate) fn new_file() -> Self {
        #[cfg(feature = "pgp")]
        helix_stdx::alloc::enable();
        Self {
            armored: false,
            passphrase: None,
            ciphertext: None,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.passphrase.is_none()
    }

    /// A file that doesn't exist yet, which needs a new passphrase.
    pub fn is_new_file(&self) -> bool {
        self.is_locked() && self.ciphertext.is_none()
    }
}

#[cfg(all(test, feature = "pgp"))]
mod tests {
    use super::*;

    const TEXT: &[u8] = b"line one\nsecret: hunter2\n";

    /// `printf 'hello\n' | gpg -c --armor`, passphrase `test123`, made with
    /// gpg 2.5.24 (SKESK v4, AES256, SEIPDv1 with MDC, ZIP compression).
    const GPG_FIXTURE: &str = include_str!("../tests/pgp/hello.txt.asc");

    #[test]
    fn roundtrip_binary_and_armored() {
        let passphrase = Passphrase::new("correct horse");
        for armored in [false, true] {
            let ciphertext = encrypt(TEXT, &passphrase, armored, "notes.txt").unwrap();
            assert_eq!(inspect(&ciphertext), Contents::Encrypted { armored });
            assert_eq!(&decrypt(&ciphertext, &passphrase).unwrap()[..], TEXT);
        }
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let ciphertext = encrypt(TEXT, &Passphrase::new("right"), false, "x").unwrap();
        assert!(decrypt(&ciphertext, &Passphrase::new("wrong")).is_err());
    }

    #[test]
    fn decrypts_gpg_output() {
        let bytes = GPG_FIXTURE.as_bytes();
        assert_eq!(inspect(bytes), Contents::Encrypted { armored: true });
        assert!(decrypt(bytes, &Passphrase::new("wrong")).is_err());
        assert_eq!(
            &decrypt(bytes, &Passphrase::new("test123")).unwrap()[..],
            b"hello\n"
        );
    }

    #[test]
    fn plain_files_are_plain() {
        assert_eq!(inspect(b"just some text\n"), Contents::Plain);
        assert_eq!(inspect(b""), Contents::Plain);
        let key_block = b"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nxsBNBGZ\n-----END PGP PUBLIC KEY BLOCK-----\n";
        assert_eq!(inspect(key_block), Contents::Plain);
    }

    #[test]
    fn paths() {
        assert!(is_pgp_path(Path::new("a/notes.md.GPG")));
        assert!(is_pgp_path(Path::new("key.asc")));
        assert!(!is_pgp_path(Path::new("notes.md")));
        assert!(is_new_file_path(Path::new("new.pgp")));
        assert!(!is_new_file_path(Path::new("new.asc")));
        assert_eq!(
            inner_path(Path::new("a/notes.md.gpg")),
            PathBuf::from("a/notes.md")
        );
        assert_eq!(inner_path(Path::new("a/b.rs")), PathBuf::from("a/b.rs"));
    }

    #[test]
    fn passphrases_are_deduplicated_newest_first() {
        let mut passphrases = Passphrases::default();
        passphrases.remember(Passphrase::new("a"));
        passphrases.remember(Passphrase::new("b"));
        passphrases.remember(Passphrase::new("a"));
        let order: Vec<_> = passphrases.iter().cloned().collect();
        assert_eq!(order, [Passphrase::new("a"), Passphrase::new("b")]);
        passphrases.forget();
        assert!(passphrases.is_empty());
    }
}
