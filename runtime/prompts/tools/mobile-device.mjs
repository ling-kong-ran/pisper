// mobile_device 的说明：经手机 App 原生桥使用用户批准的能力。

export const MOBILE_DEVICE = Object.freeze({
  description:
    'Use phone capabilities permitted by the connected Pisper mobile App and operating system, including device status, storage, clipboard, photos, sensors, and constrained external apps.',
  snippet:
    'Use capabilities permitted by the currently connected Pisper mobile App and operating system, including constrained external app launch',
  guidelines: [
    'Call mobile_device only when the user explicitly asks to use data, hardware, or an external app on their current phone.',
    'Use search_contacts for contact lookup, capture_photo for one user-visible camera capture, and get_location for one foreground location fix.',
    'Use get_capabilities before an unfamiliar mobile operation to inspect platform support, permission state, required parameters, and limitations. Use get_device_status for one complete read-only device snapshot, or use get_device_info, get_memory_status, get_storage_status, get_network_status, get_display_status, get_locale_status, and get_battery_status for focused queries. iOS does not expose global free system memory to third-party apps; report the platform limitation instead of guessing.',
    'Use get_clipboard/set_clipboard only for text the user explicitly asks to read or replace. Clipboard reads may show an operating-system privacy notice.',
    'Use list_photos to inspect metadata and create_photo_album for organization. add_photos_to_album moves Android media files and therefore requires explicit confirmation; use delete_photos only after the user explicitly confirms deletion and set confirmed=true.',
    'Photo access is limited by OS permission and Android scoped storage; never claim that every library item was inspected when the result says limited.',
    'Use vibrate and set_flashlight for small, user-requested physical feedback. Flashlight may require camera permission on both platforms.',
    'Use send_notification only for a notification the user explicitly asks Pisper to post; it is local, user-visible, and does not run background work.',
    'Use share_text for a user-visible system share sheet; opening the sheet is not confirmation that content was shared.',
    'Use open_url for HTTP(S), open_map for coordinates, open_system_settings for settings, open_dialer to prefill a phone number, and compose_sms to prefill but never send a message.',
    'Use open_app only with a known Android packageName or iOS custom-scheme appUrl. Opening an app does not authorize interaction inside it; never claim that a song, call, message, payment, or third-party action completed merely because its UI opened.',
    'A specific third-party action such as playing a song is possible only when that app documents a matching deep link or system integration. Never guess a scheme or use accessibility/UI injection.',
    'The phone may reject a capability through Pisper settings or operating-system permissions. Report that refusal without trying to bypass it.',
    'Never infer that SMS reading exists. Play-compatible Pisper builds can only open a user-visible SMS composer and do not request SMS permissions.',
    'Keep contact queries, photo limits, and asset ID batches narrow when the task does not require a broad scan.',
  ],
  params: {
    query: 'Name or phone fragment for search_contacts',
    limit: 'Maximum contacts to return',
  },
})
