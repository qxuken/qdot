//! age encryption with X25519 recipients (`master.rec`) and identities
//! (`master.key`). Files stay byte-compatible with the `age` CLI.

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::str::FromStr;

use age::{Decryptor, Encryptor, Identity, IdentityFile, Recipient};
use anyhow::{Context, Result, bail};

pub const RECIPIENTS_FILE: &str = "master.rec";
pub const IDENTITY_FILE: &str = "master.key";

pub struct Crypto {
    recipients: Vec<Box<dyn Recipient + Send>>,
    identities: Vec<Box<dyn Identity + Send + Sync>>,
    pub identity_path: Option<std::path::PathBuf>,
}

impl Crypto {
    /// Missing files are allowed: encryption then fails on first use with a
    /// clear message, decryption likewise. Modules without encrypted files
    /// never notice.
    pub fn load(recipients_file: &Path, identity_file: Option<&Path>) -> Result<Crypto> {
        let mut recipients: Vec<Box<dyn Recipient + Send>> = Vec::new();
        if recipients_file.exists() {
            let text = std::fs::read_to_string(recipients_file)
                .with_context(|| format!("reading {}", recipients_file.display()))?;
            for line in text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                let r = age::x25519::Recipient::from_str(line).map_err(|e| {
                    anyhow::anyhow!("{}: bad recipient `{line}`: {e}", recipients_file.display())
                })?;
                recipients.push(Box::new(r));
            }
        }

        let mut identities = Vec::new();
        let mut identity_path = None;
        if let Some(p) = identity_file.filter(|p| p.exists()) {
            let f = std::fs::File::open(p).with_context(|| format!("reading {}", p.display()))?;
            let file = IdentityFile::from_buffer(BufReader::new(f))
                .with_context(|| format!("parsing identity file {}", p.display()))?;
            identities = file
                .into_identities()
                .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            identity_path = Some(p.to_path_buf());
        }

        Ok(Crypto {
            recipients,
            identities,
            identity_path,
        })
    }

    pub fn can_encrypt(&self) -> bool {
        !self.recipients.is_empty()
    }

    pub fn can_decrypt(&self) -> bool {
        !self.identities.is_empty()
    }

    pub fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>> {
        if self.recipients.is_empty() {
            bail!("no age recipients: {RECIPIENTS_FILE} is missing or empty in the repo root");
        }
        let enc = Encryptor::with_recipients(
            self.recipients.iter().map(|r| r.as_ref() as &dyn Recipient),
        )
        .map_err(|e| anyhow::anyhow!("age: {e}"))?;
        let mut out = Vec::new();
        let mut w = enc.wrap_output(&mut out)?;
        w.write_all(plain)?;
        w.finish()?;
        Ok(out)
    }

    pub fn decrypt(&self, cipher: &[u8]) -> Result<Vec<u8>> {
        if self.identities.is_empty() {
            bail!(
                "no age identity: {IDENTITY_FILE} not found (set one with `qd state set-identity`)"
            );
        }
        let dec = Decryptor::new(cipher).map_err(|e| anyhow::anyhow!("age: {e}"))?;
        let mut r = dec
            .decrypt(self.identities.iter().map(|i| i.as_ref() as &dyn Identity))
            .map_err(|e| anyhow::anyhow!("age: {e}"))?;
        let mut out = Vec::new();
        r.read_to_end(&mut out)?;
        Ok(out)
    }

    /// Fresh X25519 key pair as `(identity file contents, recipient line)`.
    pub fn generate() -> (String, String) {
        use age::secrecy::ExposeSecret;
        let id = age::x25519::Identity::generate();
        let public = id.to_public().to_string();
        let secret = id.to_string().expose_secret().to_owned();
        (
            format!("# public key: {public}\n{secret}\n"),
            format!("{public}\n"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let (identity, recipient) = Crypto::generate();
        std::fs::write(tmp.path().join("k"), identity).unwrap();
        std::fs::write(tmp.path().join("r"), recipient).unwrap();
        let c = Crypto::load(&tmp.path().join("r"), Some(&tmp.path().join("k"))).unwrap();
        let cipher = c.encrypt(b"secret").unwrap();
        assert!(cipher.starts_with(b"age-encryption.org/v1"));
        assert_eq!(c.decrypt(&cipher).unwrap(), b"secret");
        let again = c.encrypt(b"secret").unwrap();
        assert_ne!(cipher, again, "ciphertext is randomized, never compare it");
    }

    #[test]
    fn missing_keys_fail_clearly() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Crypto::load(&tmp.path().join("r"), Some(&tmp.path().join("k"))).unwrap();
        assert!(!c.can_encrypt() && !c.can_decrypt());
        assert!(
            c.encrypt(b"x")
                .unwrap_err()
                .to_string()
                .contains("master.rec")
        );
        assert!(
            c.decrypt(b"x")
                .unwrap_err()
                .to_string()
                .contains("master.key")
        );
    }
}
