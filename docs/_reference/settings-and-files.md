---
title: Settings & Files
description: Settings and paths for the archive, configuration, caches, and logs.
nav_order: 0
---

## File locations

Merlin follows each platform's conventions. On Linux:

| What | Where | Safe to delete? |
| --- | --- | --- |
| Settings | `~/.config/merlin/settings.json` | Yes, you lose preferences |
| Message archive | `~/.local/state/merlin/archive.db` | Yes; only history available from WhatsApp can be restored |
| Session keys | `~/.local/state/merlin/session.db` | Yes; you must link again |
| Attachments | `~/.cache/merlin/media/` | Yes; available files download again when viewed |
| Profile pictures | `~/.cache/merlin/avatars/` | Always |
| Stickers | `~/.cache/merlin/stickers/` | Always |
| GIF search stills | `~/.cache/merlin/gifs/` | Always |
| Last run's log | `~/.local/state/merlin/merlin.log` | Always |
| Crash log | `~/.local/state/merlin/panic.log` | Always |

Back up the archive if you need its history. WhatsApp sends only recent
history to a new device, although Merlin can request some older messages from
the phone. Clearing the media cache makes Merlin download attachments again.
Expired attachments may still be available through the phone.

On macOS, settings, state, and the logs are in
`~/Library/Application Support/me.paolino.merlin` and the caches in
`~/Library/Caches/me.paolino.merlin`. On Windows, settings are in
`%APPDATA%\paolino\merlin\config`, state and the logs in
`%LOCALAPPDATA%\paolino\merlin\data`, and the caches in
`%LOCALAPPDATA%\paolino\merlin\cache`.

On first start, Merlin moves the corresponding `fastsapp` directories (or
`fastwhatsapp` from earlier versions), including the session, archive, saved
stickers, and window state. Existing Merlin directories are never overwritten.
Quit FastsApp first; launching Merlin while it is running brings the existing
window forward.

## Settings

Changes on the Settings page are saved to `settings.json` immediately:

- **Theme**: light, dark, or follow the system.
- **Enter sends**: swap Enter and Shift+Enter.
- **Download attachments automatically**: download files up to 64 MB when
  they enter view, or only when clicked.
- **Show sender pictures**: avatars next to group messages.
- **Names from your address book**: use contact names everywhere. When off,
  prefer public profile names.
- **Send read receipts**: the blue ticks others see.
- **Keep running in the background**: keep Merlin in the tray when the
  window closes.
- **Notifications**: use desktop notifications with the chat picture.
- **Check for updates**: ask GitHub once a day whether a newer release exists.
- **GIPHY API key**: required for GIF search unless the build includes one.
  Set `MERLIN_GIPHY_KEY` at compile time to include a default key.
  The earlier `FASTSAPP_GIPHY_KEY` remains a fallback for existing builds.

## The log

Each run replaces `merlin.log` and records warnings and errors. Include the
end of this file when reporting an issue.
