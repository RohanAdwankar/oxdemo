---
name: demo
description: Plan, script and record a demo video or GIF with oxdemo, for a web app or a desktop app in Docker. Use when asked to make, re-cut, shorten or fix a demo, or to write an .oxd script.
---

# Making a demo with oxdemo

## What the viewer must get

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

## Hitting a length

When asked for a length, such as 30 seconds, cut in this order:

1. Replace waits where nothing happens with `skip` (app launch, dialog
   appearing, a file opening). `skip` waits for real, then cuts that stretch
   out of the video, captions and chapters.
2. Lower `pace` (0.4 is about the floor before menus look rushed).
3. Drop whole beats that repeat an idea (a second filter, an invert).

Never cut the first scene or the app opening to hit a length.

## Writing a script for a desktop app

A remote desktop is one canvas, so targets are points: `@x,y`.

- Find points with `snap file.png`, then read the image. Snap after each step
  while exploring. Delete the snaps before committing.
- Explore in one continuous take from a fresh desktop. Reconnecting noVNC
  between takes closes open menus, so piecemeal takes mislead.
- Restart the container in `before` so every take starts identical.
- Menus: `hover` the item before you `click` it, so the highlight lands first.
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
