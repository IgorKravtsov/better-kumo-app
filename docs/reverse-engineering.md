# Reverse engineering notes

How `kumospace-api.md` and `kumospace-features.md` were produced, to redo it for a new app version.

1. Download the app: `apkeep -d apk-pure -o 'acknowledge_dangers=true' -a com.kumospace.android <dir>`.
   APKPure only ships the `armeabi-v7a` split. Keep it for static analysis only.
2. Unzip the `.xapk`, then the base `com.kumospace.android.apk`. The JS is `assets/index.android.bundle`
   (Hermes bytecode v99). `assets/app.config` holds the Expo config. Native Firebase config is in `resources.arsc`.
3. Decompile: `hbc-decompiler index.android.bundle decompiled.js` from
   [hermes-dec](https://github.com/P1sec/hermes-dec) (warns that v99 is unsupported, but the output is usable).
   Function names survive as `// Original name: X`; Expo Router files show up as `./(app)/(core)/chat/...` strings.
4. Live cross-check: the desktop app (`apps/watcher`) exposes CDP on port 9222. `Network.requestWillBeSent` and
   `performance.getEntriesByType('resource')` show the REST calls the web client makes.

Running the official app in the emulator: the arm64 Android 36 image is 64-bit only and rejects the
`armeabi-v7a` split; the Android 30 arm64 image hangs at boot on the current emulator on macOS.
Use a real device, or a Play Store image with your own account.
