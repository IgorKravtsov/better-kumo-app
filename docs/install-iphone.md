# Better Kumo on iPhone

Better Kumo isn't in the App Store. Apple runs only apps signed for your device, so you build it on a Mac or
install it with SideStore. The full guide with every option is [install.md §4](install.md#4-iphone).

## What you need

- A Mac with Xcode (from the App Store) and the build tools of [install.md §2](install.md#2-build-machine-one-time).
- An Apple ID. A free one works; the app then has to be rebuilt every 7 days, or refreshed by SideStore.
  A paid developer account ($99/year) adds push notifications and TestFlight.
  The [comparison](install.md#41-pick-an-apple-account) shows the difference.

## Build and install with a cable

1. In Xcode → Settings → Accounts, sign in with your Apple ID and note the **Team ID**.
2. Connect the iPhone by cable, unlock it and tap *Trust This Computer*.
3. On the iPhone turn on Settings → Privacy & Security → **Developer Mode** (it appears after the phone has been
   connected to Xcode once).
4. In this repository run:

   ```sh
   scripts/build-ios-device.sh --team <TEAM_ID>
   ```

5. With a free Apple ID, iOS first refuses to open the app: Settings → General → VPN & Device Management → your
   Apple ID → **Trust**.

Every 7 days (free Apple ID) run `scripts/build-ios-device.sh --team <TEAM_ID> --skip-core` again with the phone
connected. Sign-in and data are kept.

## Without the weekly trip to the Mac

SideStore re-signs the app on the iPhone itself: [install.md §4.3, SideStore](install.md#43-build-and-install).
With a paid account, TestFlight installs and updates it over the air: [install.md §4.4](install.md#44-testflight-paid-team).

## Then

1. Open Better Kumo and sign in with **Sign in with browser** ([install.md §5](install.md#5-signing-in-on-a-phone)).
2. Pair it with the computer: in the Setup guide's phone step, or Settings → Phones on the computer, tap
   **Profile** (the person icon on the Office tab) → **Scan relay QR code** and scan one code.
