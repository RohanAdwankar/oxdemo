---
name: demo
description: Plan, script and record a demo video or GIF with oxdemo, for a web app or a desktop app in Docker. Use when asked to make, re-cut, shorten or fix a demo, or to write an .oxd script.
---

# Making a demo with oxdemo

## What the viewer must get

- **The video sells the tool being demoed, not the app on screen.** In
  oxdemo's own demos, GIMP is just a stage. Every caption says what oxdemo is
  doing in that moment ("Slow waits are cut out", "It zooms in on the
  action"), never what the app is doing ("Render a plasma cloud"). Pick the
  beats in the app by which oxdemo feature they make visible.
- **Captions are big.** 52px at a 1440 wide viewport (`theme size=52`), kept
  short enough to fit on one line: about six words.
- **The first scene is simple and familiar.** Open on something the viewer
  already recognises: a plain desktop, the app's home screen. Nothing already
  open, nothing mid-task. The viewer needs to know where they are before
  anything happens. A demo that opens on a busy window with two apps already
  running overwhelms people, even when it is shorter.
- **Show how you got there.** Open the app on camera, from the menu or the
  dock, the way a person would. Never pre-launch it in `before` to save time.
  Use `skip` to cut the wait instead.
- **One idea per caption.** A short sentence a viewer can read in one glance.
  Captions hold for their reading time whatever `pace` is set to.
- **End on the payoff.** For oxdemo's own demos, that is the script that
  recorded the video, opened in an editor and zoomed so it can be read.
  Scroll through all of it with `wheel`, down to the last line, not just the
  top screenful. Clear the caption (`say ""`) before scrolling so it does not
  cover the lines.
- **Keyboard-heavy beats get `keys on`.** In a terminal or vim, the keys are
  the action, so show every one. Pause over a second after a mode key like
  vim's `i`, or it merges into the text typed after it.
- **Show breadth in separate parts.** One complete demo first (the desktop,
  30 seconds), then each extra surface (a website, a terminal) as its own
  script and its own take. Join the takes afterwards. Only the last part ends
  on its script; showing the script after every part repeats the payoff.

## Hitting a length

When asked for a length, such as 30 seconds, cut in this order:

1. Replace waits where nothing happens with `skip` (app launch, dialog
   appearing, a file opening). `skip` waits for real, then cuts that stretch
   out of the video, captions and chapters.
2. Lower `pace` (0.4 is about the floor before menus look rushed).
3. Drop whole beats that repeat an idea (a second filter, an invert).

Never cut the first scene or the app opening to hit a length.

## Writing a script for a desktop app

Target things by the words on them: `click "Graphics"`, `click "OK"`. On a
remote desktop oxdemo reads the screen to find them. Use `@x,y` only for
things with no text (an icon, a canvas, a chess square), and find those points
with `snap`.

- Find points with `snap file.png`, then read the image. Snap after each step
  while exploring. Delete the snaps before committing.
- Explore in one continuous take from a fresh desktop. Reconnecting noVNC
  between takes closes open menus, so piecemeal takes mislead.
- Restart the container in `before` so every take starts identical.
- Menus: `hover` the item before you `click` it, so the highlight lands first.
  The click reuses the hovered spot, since hovering changes how the item looks.
  From a menubar, go straight down before moving sideways. Crossing the next
  menubar label switches to that menu.
- Maximize the app first (`double-click` its title bar) so positions do not
  depend on where the window opened.
- Chrome keeps Ctrl+N, Ctrl+T, Ctrl+W and similar for itself. Use menus.
- Menus change after first use (GIMP adds "Recently Used"), so positions
  found on a fresh app can shift later in the take.

## Checking a take

1. Read `<name>-sheet.png`. Every beat should be visible and correct.
2. Pull frames from the mp4 at the moments that matter, especially the last
   seconds and any zoom, and read them.
3. Check the length: the last line of `<name>.vtt`, or ffmpeg's Duration.

Do not send a take you have not looked at.
