# Pisper Privacy Policy

Last updated: August 26, 2026

Pisper is a local-first AI workspace. This policy explains how the Pisper mobile application handles information.

## Summary

- Pisper does not require a Pisper account.
- The app does not send analytics, advertising identifiers, crash telemetry, prompts, files, contacts, photos, or location data to the Pisper developer.
- App data is stored on your device unless you choose to connect a model provider, web search provider, or your own Pisper Desktop Runtime.
- Pisper does not sell personal information and does not use personal information for advertising or tracking.

## Information stored on your device

Pisper may store the following information in the app's private storage so that its features work:

- conversations, prompts, model responses, session metadata, and app settings;
- files and images that you choose to open, create, attach, or generate;
- model and search provider configuration, including credentials that you enter;
- pairing records and access tokens for a Pisper Desktop Runtime that you control;
- optional skill, workspace, and memory data supported by the selected Runtime.

This information remains under the operating system's app sandbox. Removing the app may remove locally stored information, subject to platform backup behavior.

## Connections you choose

### AI and search providers

When you configure and use a third-party AI model or search provider, Pisper sends the content required for your request directly to that provider. This can include prompts, conversation context, selected files or images, tool results, and technical request metadata such as an IP address. The provider processes that information under its own privacy policy and account settings.

Pisper does not proxy these requests through a server operated by the Pisper developer.

### Your Pisper Desktop Runtime

You may pair the mobile app with a Pisper Desktop Runtime that you control. On a local network, the app can discover Desktop advertisements and send a connection request; the Desktop user must approve it before a device access token is issued. QR-code and manual pairing remain available when local discovery is unavailable. After pairing, the app can exchange conversations, files, settings, and Runtime results with that computer over a local-network or encrypted peer-to-peer connection. Pairing credentials are stored on the device. The Pisper developer does not receive this traffic.

## Optional device permissions

Pisper requests a device permission only when you enable or invoke the related feature:

- **Camera:** scan a pairing code or capture a photo after your action.
- **Contacts:** search contacts for an operation that you approve.
- **Location:** read a foreground location for an operation that you approve.
- **Notifications:** show local app or task notifications.
- **Local network:** discover or connect to a Pisper Desktop Runtime on your network.

Permission results and returned content are used for the requested operation. They are not sent to the Pisper developer. Content may be included in a request to a provider or Desktop Runtime only when the operation and your selected workflow require it.

You can revoke permissions in the operating system settings and disable device capabilities in Pisper.

## Updates and executable content

App Store and Google Play builds are updated only through the applicable store. Their application code, user interface, and embedded Runtime are included in the signed app package. Store builds do not download and execute plugins, scripts, Runtime replacements, or other code that changes the app's functionality.

The separately distributed GitHub build uses the same embedded Node Runtime and signed in-app user interface, but may check and open an external update channel. It does not include a rooted Runtime, `rootfs`, `su`, or `chroot` assets.

## Security and retention

Pisper uses operating-system app isolation, loopback-only services for its embedded Runtime, authenticated pairing, and certificate fingerprint validation for direct remote connections. No system is completely secure, so you should protect your device, provider credentials, and paired computers.

Local information remains until you delete it in Pisper, clear the app's storage, or uninstall the app. Information sent to a provider or stored by your Desktop Runtime is retained according to that service's settings and policy.

## Children

Pisper is not directed to children under 13, and the developer does not knowingly collect children's personal information. Provider age requirements may be higher and continue to apply.

## Changes

Material changes to this policy will be published on this page with a revised date. Store listing disclosures will be updated when the app's data practices change.

## Contact

For privacy questions or requests, contact `65328093+ling-kong-ran@users.noreply.github.com` or open an issue at <https://github.com/ling-kong-ran/pisper/issues>.
