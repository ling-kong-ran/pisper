# Mobile Store Release Runbook

The store pipeline is `.github/workflows/build-store-app.yml`. It is independent from the GitHub sideload release in `release-app.yml`. Both channels package only embedded Node and signed in-app React assets; neither publishes rooted Runtime assets.

## Required repository secrets

### Google Play

- `APP_ANDROID_KEYSTORE`: base64-encoded Java keystore containing the Play upload key.
- `APP_ANDROID_STORE_PASSWORD`: keystore password.
- `APP_ANDROID_KEY_ALIAS`: upload-key alias.
- `APP_ANDROID_KEY_PASSWORD`: upload-key password.
- `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON`: Google Play Console service-account JSON used only when upload is enabled.

Enroll the application in Play App Signing. The workflow signs the AAB with the upload key; Google manages the distribution signing key.

### Apple

- `IOS_DISTRIBUTION_CERTIFICATE`: base64-encoded Apple Distribution `.p12` file.
- `IOS_DISTRIBUTION_CERTIFICATE_PASSWORD`: `.p12` password.
- `IOS_APP_STORE_PROVISIONING_PROFILE`: base64-encoded App Store provisioning profile for `com.lingkongran.pisper`.
- `APP_STORE_CONNECT_API_KEY_ID`: App Store Connect API key ID.
- `APP_STORE_CONNECT_ISSUER_ID`: App Store Connect issuer ID.
- `APP_STORE_CONNECT_API_PRIVATE_KEY`: contents of the corresponding `.p8` private key, used only when TestFlight upload is enabled.

Tauri imports the certificate and provisioning profile into an isolated temporary keychain and exports with the `app-store-connect` method. The resulting IPA includes the signed `NodeMobile.framework`, embedded Runtime, and `PrivacyInfo.xcprivacy`.

## Build

Run **Build store apps** with:

- `version`: public semantic version, for example `1.0.0`;
- `source_sha`: exact reviewed commit to build;
- `build_number`: positive integer greater than every build previously uploaded for that platform;
- `upload_google_play`: upload the verified AAB to the Play internal track;
- `upload_testflight`: upload the verified IPA to TestFlight.

The Android job fails if the bundle contains a rooted Runtime asset or if an arm64 native library has an ELF LOAD alignment below 16 KB. The store Cargo feature also excludes `root_runtime.rs` and the GitHub update implementation from compilation.

## Submission URLs

- Privacy policy: <https://ling-kong-ran.github.io/pisper/privacy.html>
- Support: <https://ling-kong-ran.github.io/pisper/support.html>

## Review notes

State the following facts in both stores' reviewer notes:

- Node and all Runtime JavaScript are fixed resources included in the signed AAB/IPA.
- The store build does not download or execute plugins, MCP servers, shell commands, generated code, Runtime replacements, APKs, or IPAs.
- The local HTTP listener binds only to `127.0.0.1` and exists to keep the signed React UI, JSON/SSE APIs, and embedded Runtime on one authenticated origin.
- Remote mode keeps the signed React UI from the app and sends only API/data traffic to a user-controlled Pisper Desktop Runtime.
- Camera, contacts, location, notification, and local-network access are optional and are requested only for the user-selected feature.

Start with Google Play internal/closed testing and TestFlight. Resolve automated binary/privacy findings before production review, and update `store-data-disclosures.md` whenever the shipped data flow changes.
