import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

// Write a tauri.release.conf.json with release-only overrides.
//
// Tauri's --config flag merges the provided JSON on top of the base
// tauri.conf.json, so this file must contain ONLY the delta fields —
// not a copy of the base config.
//
// For OSS release builds this script emits:
// 1. bundle.macOS.minimumSystemVersion = "10.15" for broad compatibility.
// 2. bundle.createUpdaterArtifacts = true so Tauri produces the .tar.gz
//    archive and .sig signature during the build.
// 3. plugins.updater with the public key and endpoint from env vars.
//    Both BUZZ_UPDATER_PUBLIC_KEY and BUZZ_UPDATER_ENDPOINT are required -
//    the script fails if either is missing (OSS builds always ship with updater).
//
// Apple code signing and notarization happen post-build via
// block/apple-codesign-action in release.yml, so no signingIdentity is
// emitted here and the Tauri build is invoked with --no-sign.
//
// FORK-LOCAL PATCH (adrienlacombe/buzz): BUZZ_MACOS_ADHOC_SIGN=1 additionally
// emits bundle.macOS.signingIdentity = "-", i.e. ad-hoc signing.
//
// Needed because a lane with no Apple identity produces an .app with no
// Contents/_CodeSignature at all — only the linker's ad-hoc signature on the
// executable. macOS reads that mismatch as corruption and refuses the app with
// "is damaged and can't be opened", which no amount of clearing the quarantine
// attribute fixes. Every macOS build this fork had ever produced was affected,
// release and canary alike; it went unnoticed until someone installed one.
//
// It must be config rather than a `codesign` step after the build, because Tauri
// assembles the DMG inside the same `tauri build` invocation. Signing afterwards
// would fix the .app on disk while leaving the DMG — the thing users download —
// carrying the unsigned copy.
//
// Off by default so the two block/buzz lanes are untouched: they must reach
// block/apple-codesign-action unsigned. Set it only where nothing else will ever
// sign the bundle.

const outputConfigPath = resolve(
  process.cwd(),
  "src-tauri/tauri.release.conf.json",
);

const updaterPubkey = process.env.BUZZ_UPDATER_PUBLIC_KEY;
const updaterEndpoint = process.env.BUZZ_UPDATER_ENDPOINT;

const missing = [];
if (!updaterPubkey) missing.push("BUZZ_UPDATER_PUBLIC_KEY");
if (!updaterEndpoint) missing.push("BUZZ_UPDATER_ENDPOINT");
if (missing.length > 0) {
  console.error(
    `Error: required environment variable(s) missing: ${missing.join(", ")}`,
  );
  process.exit(1);
}

// FORK-LOCAL PATCH (adrienlacombe/buzz): see the header note.
const adhocSign = process.env.BUZZ_MACOS_ADHOC_SIGN === "1";

const releaseConfig = {
  bundle: {
    macOS: {
      minimumSystemVersion: "10.15",
      ...(adhocSign ? { signingIdentity: "-" } : {}),
    },
    createUpdaterArtifacts: true,
  },
  plugins: {
    updater: {
      pubkey: updaterPubkey,
      endpoints: [updaterEndpoint],
    },
  },
};

// Tauri applies --config after platform-specific config using RFC 7396.
// Any externalBin value here would therefore replace the platform sidecar list,
// while null would silently delete it. This delta must never own that key.
if (Object.hasOwn(releaseConfig.bundle, "externalBin")) {
  throw new Error(
    "Release config must not define bundle.externalBin; sidecars are platform-specific",
  );
}

console.log(`Updater enabled -> ${updaterEndpoint}`);
console.log(
  adhocSign
    ? "macOS bundle signing: ad-hoc (signingIdentity '-')"
    : "macOS bundle signing: none emitted (expects a post-build signer)",
);

writeFileSync(outputConfigPath, `${JSON.stringify(releaseConfig, null, 2)}\n`);
console.log(`Wrote ${outputConfigPath}`);
