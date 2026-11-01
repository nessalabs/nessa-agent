# Setup passphrase blocklist

`common-passphrases.txt` contains the lowercase, trimmed, unique entries of at
least fifteen Unicode characters from SecLists' public one-million common
password list. The input is public test/security data, not Nessa user data.

Source: https://github.com/danielmiessler/SecLists/blob/master/Passwords/Common-Credentials/xato-net-10-million-passwords-1000000.txt
Snapshot: 2026-10-31. Input SHA-256: `424a3e03a17df0a2bc2b3ca749d81b04e79d59cb7aeec8876a5a3f308d0caf51`.
License: [SecLists MIT](LICENSE-SecLists).

The domain setup rule consumes this file directly. Shorter entries cannot pass
the separately published setup length floor and were excluded to keep the asset
small. Repetition checks cover repeated short or long strings. This finite list
does not promise to recognize every compromised or guessable passphrase.
Unlock does not consult the blocklist, so previously accepted input remains usable.
