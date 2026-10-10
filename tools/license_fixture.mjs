// Writes the test fixture for src/license.rs: a license file signed exactly the way the
// license server signs, with a throwaway key.
// Usage: node tools/license_fixture.mjs
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { writeFileSync } from 'node:fs';

const canonical = (v) => {
  if (v === null || typeof v !== 'object') return JSON.stringify(v ?? null);
  if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`;
  return `{${Object.keys(v).filter((k) => v[k] !== undefined).sort().map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`).join(',')}}`;
};
const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const iat = 1791504000;
const payload = {
  v: 1, typ: 'app-license', app: 'leanfps', lid: 'lic-1', aid: 'act-1', plan: 'pro', plan_name: 'LeanFPS Pro',
  email_hint: 'p***@example.com', device: createHash('sha256').update('leanfps\nwin:11111111-2222-3333-4444-555555555555').digest('hex'),
  seats: 3, max_major: 1, features: { note: 'é "quoted"\n' }, iat, refresh_after: iat + 7 * 86400, exp: iat + 30 * 86400,
};
const sig = sign(null, Buffer.from('artisales.app-license.v1\n' + canonical(payload), 'utf8'), privateKey).toString('base64url');
writeFileSync(new URL('./license_fixture.json', import.meta.url), JSON.stringify({ payload, kid: 'lic-test', alg: 'Ed25519', sig }, null, 2) + '\n');
writeFileSync(new URL('./license_fixture.pub', import.meta.url), publicKey.export({ format: 'jwk' }).x + '\n');
