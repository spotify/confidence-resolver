import { readFile, writeFile } from 'node:fs/promises';

// Wrap the checked-in raw ResolverState in the CDN ClientResolverState envelope.
const state = await readFile('../data/resolver_state_current.pb');
const length = [];
let remaining = state.length;
while (remaining >= 128) {
  length.push((remaining & 127) | 128);
  remaining >>>= 7;
}
length.push(remaining);
await writeFile('../data/resolver_state_current.pb', Buffer.concat([
  Buffer.from([0x0a, ...length]), state,
  Buffer.from([0x12, 0x0c]), Buffer.from('test-account'),
]));
