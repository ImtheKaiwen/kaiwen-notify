// Writes the release-only Tauri config overlay that enables the updater.
// Usage: UPDATER_PUBKEY=... UPDATER_ENDPOINT=https://.../latest.json node scripts/make-updater-config.mjs [out]
// Nothing secret is written: the public key is public; the private key only signs at build time.
import { writeFileSync } from 'node:fs';
const pubkey = process.env.UPDATER_PUBKEY?.trim();
const endpoint = process.env.UPDATER_ENDPOINT?.trim();
if (!pubkey || !endpoint) { console.error('UPDATER_PUBKEY and UPDATER_ENDPOINT are required.'); process.exit(1); }
if (!/^https:\/\//.test(endpoint) && !/^http:\/\/(localhost|127\.0\.0\.1)[:/]/.test(endpoint)) { console.error('Endpoint must be https (http only for localhost tests).'); process.exit(1); }
const config = {
  bundle: { createUpdaterArtifacts: true },
  plugins: { updater: { pubkey, endpoints: [endpoint], windows: { installMode: 'passive' },
    ...(endpoint.startsWith('http://') ? { dangerousInsecureTransportProtocol: true } : {}) } },
};
writeFileSync(process.argv[2] ?? 'src-tauri/tauri.release.conf.json', JSON.stringify(config, null, 2));
