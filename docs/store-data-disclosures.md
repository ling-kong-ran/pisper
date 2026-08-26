# Mobile Store Data Disclosures

This checklist documents the intended declarations for the signed `mobile-store` builds. Re-run the checklist before every submission and whenever dependencies, permissions, telemetry, accounts, payments, or network behavior change.

## Product data flow

| Data or capability | App behavior | Sent to Pisper developer | Possible external recipient |
| --- | --- | --- | --- |
| Prompts, responses, session content | Stored in app-private Runtime data | No | User-selected AI provider; paired Desktop Runtime |
| Files and images | Used only when selected or generated | No | User-selected provider or paired Desktop Runtime when required by the request |
| Provider credentials | Stored in app-private Runtime data | No | Credential's provider during authenticated requests |
| Desktop pairing token and endpoint | Stored in app-private settings | No | User-controlled Desktop Runtime |
| Camera | Pairing scan or user-approved capture | No | Only the selected Runtime/provider if the user submits the result |
| Contacts | User-approved contact search | No | Only the selected Runtime/provider if the user submits the result |
| Precise location | One foreground reading after approval | No | Only the selected Runtime/provider if the user submits the result |
| Local network | Discover a user-controlled Desktop and request an approved connection | No | User-controlled Desktop Runtime |
| Diagnostics/analytics/advertising ID | No developer telemetry or ad SDK | No | None |

## Google Play Data safety draft

Recommended answers for the current store build:

- Data collected by the developer: **No**.
- Data shared by the developer: **No**.
- Data encrypted in transit: **Yes** for provider HTTPS, authenticated TLS/peer-to-peer remote connections. Loopback traffic remains on-device.
- Account creation: **No Pisper account**.
- Data deletion request mechanism: **Not applicable for developer-held data**; explain local deletion and provider/Desktop Runtime deletion in the privacy policy.
- Ads: **No**.
- Location: foreground precise location is optional and requested only for a user-approved operation; no background location.
- Contacts, photos/files, and camera content: user-initiated, processed locally or sent to a user-selected service; not collected by the developer.

Google's definitions can treat some transfers to third-party providers as sharing unless they qualify as user-initiated transfers or service-provider processing. Confirm the final form against the exact provider features enabled in the submitted binary and document the applicable exemption in Play Console.

## Apple App Privacy draft

Recommended App Store Connect response for the current store build:

- **Data Not Collected** by the developer.
- Tracking: **No**.
- Data linked to the user by the developer: **No**.
- Third-party advertising or analytics SDKs: **None**.

The privacy policy must still explain that content is sent directly to services the user configures. If a future default provider, hosted relay, analytics SDK, crash reporter, account service, or developer-operated backend receives data, replace `Data Not Collected` with the precise categories and purposes before uploading that build.

## Permission and listing checklist

- Publish `docs/privacy.html` and `docs/support.html` at stable public HTTPS URLs.
- Privacy URL: <https://ling-kong-ran.github.io/pisper/privacy.html>.
- Support URL: <https://ling-kong-ran.github.io/pisper/support.html>.
- Put the privacy URL in Google Play Console and App Store Connect.
- Put the support URL in App Store Connect.
- Describe camera, contacts, foreground location, notifications, and local-network access in the listing and review notes.
- Include `PrivacyInfo.xcprivacy` in the iOS app bundle and validate all required-reason API declarations from Apple and included SDKs.
- Complete Google Play's App access, Content rating, Target audience, Data safety, Ads, and sensitive-permission forms.
- Complete Apple's App Privacy, age rating, export compliance, content rights, and review contact fields.
- Provide review notes stating that Node and Runtime JavaScript are fixed, signed app resources; the store build does not download or execute code, plugins, MCP servers, shell commands, or Runtime updates.
- Provide a test provider configuration or review account only through the stores' secure reviewer fields, never in the repository or review notes visible to users.
