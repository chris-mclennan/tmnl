-- Drive the dev tmnl through the native-tab demo.
--
-- Arguments:
--   $1  pid of the dev tmnl process to drive (recording target)
--   $2  digit to press at the first welcome overlay (e.g. "2" for mnml)
--   $3  digit to press at the second welcome overlay (e.g. "2" for mnml again)
--
-- Choreography (T=0 is when screencapture is already rolling):
--   T+0  s : pause -- viewer reads welcome overlay
--   T+2.0s : keystroke <digit_first>  -- opens first native tab (mnml)
--   T+8.0s : keystroke Cmd+T          -- new shell tab; welcome appears
--   T+10.0s: keystroke <digit_second> -- opens second native tab
--   T+16.0s: keystroke Cmd+Shift+[    -- switch to first tab
--   T+19.0s: keystroke Cmd+Shift+]    -- switch back to second tab
--   T+22.0s: keystroke Cmd+Shift+[    -- one more switch for clarity
--   T+25.0s: end
--
-- Total runtime: ~25 s. screencapture -V 28 in the shell wrapper gives
-- a small buffer at the head/tail.

on run argv
    set targetPid to item 1 of argv as integer
    set d1 to item 2 of argv
    set d2 to item 3 of argv

    -- Activate the dev tmnl by pid. There may be two tmnl processes
    -- (the bundled .app + this dev binary); we activate the specific one.
    tell application "System Events"
        set targetProc to first process whose unix id is targetPid
        set frontmost of targetProc to true
    end tell
    delay 0.4

    -- T+0 -> T+2: hold on welcome overlay
    delay 2.0

    -- T+2: press first digit -> mnml opens as native tab
    tell application "System Events" to keystroke d1
    -- Allow mnml to load + first frame to render
    delay 6.0

    -- T+8: Cmd+T -> new shell tab, welcome re-appears on it
    tell application "System Events" to keystroke "t" using {command down}
    delay 2.0

    -- T+10: press second digit -> second mnml native tab
    tell application "System Events" to keystroke d2
    -- Allow second mnml to load
    delay 6.0

    -- T+16: Cmd+Shift+[ -> previous tmnl tab (back to first mnml)
    tell application "System Events" to keystroke "[" using {command down, shift down}
    delay 3.0

    -- T+19: Cmd+Shift+] -> next tmnl tab (forward to second mnml)
    tell application "System Events" to keystroke "]" using {command down, shift down}
    delay 3.0

    -- T+22: Cmd+Shift+[ once more
    tell application "System Events" to keystroke "[" using {command down, shift down}
    delay 3.0
end run
