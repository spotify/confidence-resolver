#!/usr/bin/env node
// Decrypts AES-256-GCM encrypted resolver state (Tink NO_PREFIX format).
// Usage: node decrypt_state.js <input_file> <hex_key> [output_file]
//   If output_file is omitted, decrypted data is written back to input_file.
const crypto = require('crypto');
const fs = require('fs');

const [,, inputFile, hexKey, outputFile] = process.argv;
if (!inputFile || !hexKey) {
  console.error('Usage: node decrypt_state.js <input_file> <hex_key> [output_file]');
  process.exit(1);
}

const encrypted = fs.readFileSync(inputFile);
const key = Buffer.from(hexKey, 'hex');
const nonce = encrypted.subarray(0, 12);
const tag = encrypted.subarray(encrypted.length - 16);
const ciphertext = encrypted.subarray(12, encrypted.length - 16);
const decipher = crypto.createDecipheriv('aes-256-gcm', key, nonce);
decipher.setAuthTag(tag);
const plaintext = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
fs.writeFileSync(outputFile || inputFile, plaintext);
