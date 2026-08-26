# Pisper Mobile Support

Pisper is a local-first AI workspace for Android and iOS.

## Get help

Before reporting a problem, record the Pisper version, operating-system version, device model, whether you are using the on-device Runtime or a paired Desktop Runtime, and the exact error message.

- Issues and feature requests: <https://github.com/ling-kong-ran/pisper/issues>
- Source and release information: <https://github.com/ling-kong-ran/pisper>
- Privacy policy: [Pisper Privacy Policy](privacy.md)
- Contact: `65328093+ling-kong-ran@users.noreply.github.com`

Do not include API keys, pairing tokens, private prompts, personal files, contact details, or other sensitive information in a public issue.

## Common checks

### Model requests fail

Confirm that the selected provider is reachable, the provider credential is valid, and the model name is available to your account. Provider traffic goes directly from your selected Runtime to that provider.

### Desktop pairing fails

Confirm that Pisper Desktop is running and remote access is enabled. On the same LAN, allow local-network access in the mobile operating-system settings, select the discovered Desktop, and approve the request on Desktop. If discovery is unavailable or the devices are in different locations, scan a newly generated QR code or enter its address, pairing code, and TLS fingerprint manually.

### A device capability is unavailable

Enable the capability in Pisper, then grant the corresponding camera, contacts, location, notification, or local-network permission in system settings. Pisper continues to work with reduced functionality when optional permissions are denied.

### Delete local data

Delete individual sessions and files in Pisper where available. To remove all app-local data, use the operating system's app storage controls or uninstall Pisper. Data sent to an AI/search provider or stored by a paired Desktop Runtime must be deleted through that provider or Runtime.

## Store purchases

Pisper currently does not sell subscriptions or in-app purchases. Provider usage and billing are governed by the provider account that you configure.
