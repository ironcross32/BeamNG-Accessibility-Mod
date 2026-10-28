# Lua: drift display speech (`driftAccessible.lua`)

`driftAccessible.lua` speaks the stock drift display. It relays the game's drift hooks to Python as `drift_event` frames over `bnvdaBridge`, the same path `hillClimbChallenge.lua` uses. The events are few and need no fresh-data stream, so no UDP port is used. `drift_speech.py` turns them into speech; see [python-components.md](python-components.md).

## How the game's drift system works

All of this was read from `lua/ge/extensions/gameplay/drift/` and `ui/modules/apps/Drift*`.

- `general.lua` holds a **context**, and every change is broadcast as `onDriftContextChanged(context, old)`:
  - `inFreeroam`: free roam with no display.
  - `inFreeroamCruising`: the free-roam drift display. It needs the game setting `enableDriftFreeroamCruising` ("show drift UI in free roam"). `freeroam/cruising.lua` enters it after 1.5 s of drifting with 3 or more linked slides, or one 3.2 s slide covering 25 m or more. It leaves 3 s after the last slide.
  - `inFreeroamChallenge`: a drift spot run.
  - `display.lua` and `sounds.lua` load only in the display contexts. So "the drift UI shows" means the context is one of those two.
- **The free-roam display is the simplified current-drift app** (`DriftCurrentDrift`, simplified when the stream's context is `inFreeroamCruising`) plus `DriftScores`. It shows:
  - the points and combo ×N;
  - a quick-message line, for example "Close call! +80";
  - "Crashed!" or "Spinout!";
  - the confirmed "+N" when a chain is banked;
  - the tier flash ("Drift!" through "Legendary Drift!");
  - "Pts: total".
- **Every message the display shows has a GE hook**, raised with `extensions.hook`, so any loaded extension hears it. `onDriftStatusChanged`, `onDriftNewCombo`, `onNewDriftTierReached`, `onDriftQuickMessageDisplay`, `onDriftScoreChanged`, `onDriftCrash`, `onDriftSpinout`, `onDriftCompletedScored`, `onDriftScoreWrappedUp`, the `onDriftSpot*` hooks and `onFreeroamDriftZoneNewHighscore`.
- **Out of bounds has no hook.** `display.lua` polls `gameplay_drift_bounds.getIsOutOfBounds()`, and so does this extension, edge-triggered, only in `inFreeroamChallenge`.

## Rules

- **Events are sent only while the display shows**, so the speech mirrors the screen. Quick messages and tiers raised in plain `inFreeroam` have no display and are not spoken. The tier hook fires "Drift!" at every chain start, `scoring.lua:762`.
- **Wrong way is not polled.** `onDriftWrongWay` fires every frame. In a drift spot the display shows no wrong-way message; the spot ends with `onDriftSpotFailed`, and that reason ("Drift terminated: You drove back up the course") is what gets spoken.
- **Points lost on a crash or spinout are the last `potentialScore` seen.** Hook order between extensions is not fixed, so when the fail hook arrives scoring may already have reset the cached score. **`onDriftCachedScoreReset` must not clear it.** Scoring's own fail handler raises that reset hook, nested, before our fail hook runs. Clearing there made the second driven run's spinout say no points lost. It never goes stale either: scoring raises `onDriftScoreChanged` with 0 the frame after any reset.
- **A fail also reads the session total.** The display keeps "Pts: total" up through a crash or spinout, and no "+N" follows, so the total would otherwise go unsaid ("Spinout! 640 points lost. Total 18,916").
- **Hooks carrying a `vehId` are filtered to the player vehicle.** The game tracks every vehicle in splitscreen and multiplayer.
- **Text comes from the game's own translations** (`translateLanguage`), with `{{score}}` placeholders filled. Each has an English fallback, so a renamed key degrades to plain words rather than a key name.
- **Every hook body runs under `pcall`.** They are called from inside the game's drift update.
- **A reload adopts the current context silently.** A reload mid-drift is not a new display.
- **A silent `alive` goes out every 3 s while shown.** Python's quiet window times out 10 s after the last event, in case `hidden` is lost. Without the keepalive, one long slide would fall out of it.

## Drift spots

`onDriftSpotStarted` carries only a `lineId`. The name, best score and objectives are looked up through `gameplay_drift_saveLoad.getDriftSpotsById()`, searching each spot's `lines` for that id. If the lookup fails, it falls back to "Drift zone started". **That lookup shape has not been confirmed in game yet.** Endless Surfaces has no spots; check it on a level that does.

## First driven run, Endless Surfaces

- Three chains were spoken in order, from "Drift. 638 points, combo 1.2" to "Drift over".
- No automatic speed or compass speech came in the 26 s window.
- Gear changes were spoken seven times during the drift, and each one interrupted. "second" came 0.2 s after a burst of four drift lines. During the window, gear speech now queues instead (see `drift_speech.py` in python-components.md).
- "Big angle" and "Reverse drift" repeated across two slides. That is the game: both reset at each drift end, and the display showed them twice.

## Third driven run, after the fixes

- The spinout read "Spinout! 538 points lost. Total 8,720".
- Six banked chains added up correctly.
- Every shift inside the window queued (`interrupt` false). The first shift after "Drift over" interrupted again.
- No automatic speed or compass speech came inside the window.

## Checks

- `diagnostic/drift_speech_sim.py` covers phrases, interrupts, the quiet window, the keepalive, the timeout and the setting.
- In game, the handlers were called directly with fake data and `bnvdaBridge.sendFromGE` stubbed. They produced the expected frames, filtered another vehicle's slide, and translated a spot end reason. Never test with `extensions.hook`, which also drives the game's own drift modules.
