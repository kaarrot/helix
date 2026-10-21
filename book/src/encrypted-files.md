## Encrypted files

- [Building with encryption support](#building-with-encryption-support)
- [Opening](#opening)
- [Saving](#saving)
- [New files](#new-files)
- [Passphrases](#passphrases)
- [What stays private](#what-stays-private)

Helix edits files encrypted with a passphrase, the ones `gpg -c` (or
`gpg --symmetric`) makes, without calling `gpg`. It reads and writes the
OpenPGP format itself.

Files encrypted to a public key (`gpg -e`, as `pass` does) are not
supported: opening one shows an error.

### Building with encryption support

Encryption support is a build option, the `pgp` Cargo feature, and is off by
default. Build Helix with it:

```sh
cargo install --path helix-term --locked --features pgp
```

or, to build without installing:

```sh
cargo build --release --bin hx --features pgp
```

It adds the [rPGP](https://github.com/rpgp/rpgp) crate and its pure-Rust
cryptography dependencies, which need Rust 1.88 or newer. A Helix built
without the feature opens `.gpg` files as it always did, as text, and has no
`:pgp-forget` command. `hx --version` doesn't show which way it was built; open
an encrypted file to check.

### Opening

A `.gpg`, `.pgp` or `.asc` file holding a passphrase-encrypted message opens
read-only and empty, and Helix asks for its passphrase. The input shows a `*`
for each character.

- With the right passphrase the text appears. There is nothing to undo yet.
- A wrong one asks again, saying it was wrong.
- <kbd>Esc</kbd> closes the file.

The extension is ignored when detecting the language, so `notes.md.gpg` is
highlighted as Markdown. Other `.gpg` and `.asc` files, such as keys and
signatures, open as text as before.

### Saving

Saving encrypts the text again with the file's passphrase, also when writing
it under another name. The file stays ASCII armored if it was. Before
anything is written, the output is decrypted again and compared with the
text, so a file that can't be read back is never saved.

`:reload` decrypts the file again with its passphrase.

### New files

Opening a `.gpg` or `.pgp` file that doesn't exist asks for a new passphrase,
twice, and starts over if the two don't match. A new `.asc` file stays plain text, as it is more likely a key or a
signature.

Helix refuses to write a plain buffer to a new `.gpg` or `.pgp` file. Open
the file instead, to give it a passphrase.

### Passphrases

A passphrase that opens a file is remembered until Helix exits, and is tried
first on every encrypted file opened later.

| Command | Description |
| --- | --- |
| `:pgp-forget` | Forget the passphrases remembered for encrypted (gpg -c) files this session. |

### What stays private

While an encrypted file is open, its text stays inside Helix: it isn't sent
to language servers or formatters, it gets no git diff gutter, and word
completion doesn't offer its words in other buffers.

When the file is closed:

- Once an encrypted file has been opened, Helix overwrites every block of
  memory with zeros as it frees it, so the copies of the text it made (the
  buffer, its undo history, syntax tree and the like) are wiped.
- Registers that got text while the file was focused are cleared, except the
  command history (`:`).
- The system clipboard is cleared if it still holds text copied from the
  file. This is also checked when Helix exits.

Some copies are out of Helix's reach:

- memory the operating system moved to swap while the file was open,
- the terminal's own copy of the screen and its scrollback (tmux too),
- clipboard managers and keyboard clipboard histories,
- a crash dump.
