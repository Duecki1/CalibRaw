# CalibRaw App Privacy Policy

Last updated: October 16, 2026

## 1. Controller

Controller within the meaning of the EU General Data Protection Regulation (GDPR):

Linus Dückerhoff
c/o Online-Impressum 11794
Europaring 90
53757 Sankt Augustin
Germany
Email: calibraw@dueckis.de

## 2. Scope

This privacy policy applies to the **CalibRaw app** on Android, Linux, Windows and macOS. It describes how the app handles user and device data.

## 3. Principle: All processing happens on your device

CalibRaw is a local RAW photo editor. Your photos, RAW files, edits (`.calibraw` sidecars), presets and exports are **processed and stored exclusively on your device**. There is no user account, no cloud sync, no analytics or tracking services, and no advertising.

CalibRaw does **not** collect any of the following data:

- Name, email address or other contact details
- Location data
- Photos, RAW files or exports (they are never transmitted)
- Device identifiers or advertising IDs
- App usage behavior, clicks or interactions
- Diagnostic, crash or performance data
- Payment information

## 4. Internet connections: Only with your consent

By default, CalibRaw works fully offline. The app only connects to the internet for the following features, and only after you have agreed to them in the app. Without your consent, no connection is made, and all other features work without an internet connection.

### 4.1 Downloading AI models and the ONNX Runtime

If you use the AI features (subject, sky and object masks, depth masks, AI Remove, AI Denoise), CalibRaw downloads the required model files and, on desktop platforms, the ONNX Runtime once from our model repository on **Hugging Face** (Hugging Face, Inc., USA).

- **What is transmitted:** During the download, your **IP address**, the time of the request and the requested file are technically transmitted to Hugging Face. This is required to deliver the file.
- **What is not transmitted:** CalibRaw does not send any device identifier, user identifier, or any information about your photos or your use of the app.
- **Recipients:** Hugging Face and the content delivery network (CDN) partners it uses to deliver the files. They process the access data under their own privacy policy: https://huggingface.co/privacy
- **Verification:** Downloaded files are verified via SHA-256 checksum and then run locally and offline.
- **Legal basis:** Art. 6 (1) (a) GDPR (your consent).

### 4.2 Update check (GitHub)

CalibRaw can check whether a newer version has been released. The check asks the public GitHub API (GitHub, Inc., USA) for the latest release of CalibRaw. It only runs after you have allowed it, either at startup or when you start it yourself in Settings, and you can turn it off at any time in Settings. Builds distributed through F-Droid and Flathub do not contain this check, because those stores deliver updates themselves.

- **What is transmitted:** Your **IP address**, the time of the request and standard request data (such as the app name and version in the request header).
- **What is not transmitted:** No device identifier, user identifier, or information about your photos or your use of the app.
- **Recipient:** GitHub, Inc., under its own privacy statement: https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement
- **Legal basis:** Art. 6 (1) (a) GDPR (your consent).

If an update is available, CalibRaw shows a link to the release page. Opening it happens in your browser.

### 4.3 Transfers to the USA

Hugging Face and GitHub are based in the USA. Where a provider is certified under the EU-U.S. Data Privacy Framework, the transfer is based on the European Commission's adequacy decision (Art. 45 GDPR); otherwise it is based on the provider's Standard Contractual Clauses (Art. 46 GDPR) and your consent (Art. 49 (1) (a) GDPR). US authorities may be able to access data held by these providers.

CalibRaw does not offer any way to make payments through the app.

## 5. Discord Rich Presence (desktop only, optional)

On Windows, macOS and Linux (except the Flatpak version), you can turn on Discord Rich Presence. It is **off by default**. When it is on, CalibRaw sends your locally running Discord app whether you are browsing the library or editing, and how long the current edit session has lasted. Discord then shows this status to other Discord users according to your Discord settings. Photo names, file paths and image content are never sent. CalibRaw itself does not connect to Discord's servers; the Discord app does that under Discord's privacy policy: https://discord.com/privacy

Legal basis: Art. 6 (1) (a) GDPR (your consent by turning the setting on). You can turn it off at any time in Settings.

## 6. Google Play Store

If you download CalibRaw from the Google Play Store, **Google Ireland Limited** processes data in the process (e.g. device data, Google account information). This is governed by Google's privacy policy: https://policies.google.com/privacy. CalibRaw has no influence over this processing.

## 7. App permissions

### Android

CalibRaw does not request general access to your photos, media or files. You choose individual files and folders yourself with Android's system file picker, and CalibRaw only accesses what you have selected. Exports are saved to your media library through Android's media store.

CalibRaw requests these permissions:

- **Network access (`INTERNET`):** used only for the connections described in section 4.
- **Notifications (`POST_NOTIFICATIONS`):** shows the progress of long-running tasks such as exports while the app is in the background. Notifications are created locally on your device; no data is sent. You can deny or revoke this permission at any time.
- **Storage (`WRITE_EXTERNAL_STORAGE`):** requested only on Android 9 and older, where it is required to save exports.

### Desktop platforms

No special system permissions are required. CalibRaw only accesses the files and folders you open or choose yourself, and the camera profiles described in section 8.

## 8. Camera profiles (DCP)

CalibRaw can use DCP camera profiles for consistent color rendering:

- **Windows and macOS:** CalibRaw can find DCP profiles that are already installed on your device (e.g. from an existing Adobe Lightroom installation) and read them locally.
- **Android:** If you choose a profile folder with the system file picker, CalibRaw copies the `.dcp` files from it into the app's private storage so they stay available.

Camera profiles are never uploaded or otherwise transmitted.

## 9. External links

CalibRaw contains links to the project's GitHub page and to the source of the downloaded models and runtime. When you open a link, it opens in your browser, and the privacy policy of the respective provider applies. The app itself does not embed any web views.

## 10. Audience

CalibRaw is not directed at children. The app is not marketed to children under 13 and knowingly collects no data from children.

## 11. Storage duration and deletion

All data created by CalibRaw is stored exclusively on your device and can be deleted there at any time. CalibRaw never stores data about you on any server.

- **App data** (settings, presets, previews, model cache, imported camera profiles) is removed when you uninstall the app. On Android, this data is not included in Android backups.
- **Files you save outside the app** – exports in your media library or folders, and `.calibraw` sidecars saved next to your photos – are your files. They remain on your device after uninstalling and can be deleted like any other file.

## 12. Your rights under the GDPR

CalibRaw has no user accounts and we do not store any data about you. The only processing operations are the optional connections described in sections 4 and 5 and the store download described in section 6. To the extent the GDPR applies, you have the following rights against the controller:

- **Access** (Art. 15 GDPR)
- **Rectification** (Art. 16 GDPR)
- **Erasure** (Art. 17 GDPR)
- **Restriction of processing** (Art. 18 GDPR)
- **Data portability** (Art. 20 GDPR)
- **Withdrawal of consent** at any time with effect for the future (Art. 7 (3) GDPR) – for example, by turning off the update check or Discord Rich Presence in Settings, or by not downloading further models

You also have the right to lodge a complaint with a data protection supervisory authority (Art. 77 GDPR), in particular the supervisory authority of the German state where you live.

Since no data is stored by us, access or deletion requests usually cannot be fulfilled by us directly; they concern the respective third parties (Hugging Face, GitHub, Discord, Google). If you have any questions, feel free to contact the email address above – we are happy to help.

## 13. Changes to this policy

This privacy policy may be updated when the app's features change. The current version is permanently available at https://github.com/Duecki1/CalibRaw/blob/main/privacypolicy.md

## 14. Contact

Privacy questions: calibraw@dueckis.de
