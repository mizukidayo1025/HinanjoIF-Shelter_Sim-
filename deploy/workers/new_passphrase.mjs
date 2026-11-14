// Makes a passphrase for one recipient of the LLM relay (relay.mjs, #419) and the entry of the
// secret LLM_PASSPHRASES that lets it in:
//
//   node deploy/workers/new_passphrase.mjs 審査員A
//
// prints the passphrase (give it to the recipient; it is not kept anywhere) and the entry
// { label, sha256 } to add to the JSON array of the secret. The passphrase is 20 random
// characters (about 100 bits), so an unsalted SHA-256 of it is enough to keep in the secret.

import { createHash, randomInt } from "node:crypto";

const label = process.argv[2];
if (!label || label.length > 40) {
  console.error("usage: node deploy/workers/new_passphrase.mjs LABEL  (a name of at most 40 characters)");
  process.exit(2);
}
// Letters and digits that are not easily mistaken for one another.
const ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789";
let passphrase = "";
for (let i = 0; i < 20; i++) {
  if (i > 0 && i % 5 === 0) passphrase += "-";
  passphrase += ALPHABET[randomInt(ALPHABET.length)];
}
const sha256 = createHash("sha256").update(passphrase, "utf8").digest("hex");
console.log(`passphrase: ${passphrase}`);
console.log(`entry:      ${JSON.stringify({ label, sha256 })}`);
