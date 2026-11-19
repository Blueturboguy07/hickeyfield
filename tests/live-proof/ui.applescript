-- UI helpers for tests/live-proof/proof-macos.sh, through System Events. Runner only.
--
-- Hickeyfield's questions (the publik sign-in gate and the update questions) are message dialogs
-- of tauri-plugin-dialog with no parent window. rfd shows them with CFUserNotificationDisplayAlert,
-- so macOS draws each one as a window of the UserNotificationCenter process, not of the app.
--
--   osascript ui.applescript dump                       every UserNotificationCenter window: texts and buttons
--   osascript ui.applescript find <text>                the texts of the first window whose texts contain <text>, or ""
--   osascript ui.applescript press <text> <button>      clicks <button> in that window
--   osascript ui.applescript menu <bundle id> <prefix>  brings the app forward and clicks the first item of its
--                                                       app menu whose name starts with <prefix>

on run argv
	set cmd to item 1 of argv
	if cmd is "dump" then return my dumpAlerts()
	if cmd is "find" then return my findAlert(item 2 of argv, "")
	if cmd is "press" then return my findAlert(item 2 of argv, item 3 of argv)
	if cmd is "menu" then return my clickMenu(item 2 of argv, item 3 of argv)
	error "unknown command " & cmd
end run

on joined(theList, sep)
	set t to ""
	repeat with x in theList
		try
			set s to (contents of x) as text
		on error
			set s to "?"
		end try
		if t is "" then
			set t to s
		else
			set t to t & sep & s
		end if
	end repeat
	return t
end joined

-- {texts, button names, button elements} of a window, from its whole element tree.
on describeWindow(w)
	set theTexts to {}
	set theNames to {}
	set theButtons to {}
	tell application "System Events"
		try
			set els to entire contents of w
		on error
			set els to {}
		end try
		repeat with e in els
			try
				set c to class of e
				if c is static text or c is text area or c is text field then
					set v to value of e
					if v is not missing value then set end of theTexts to (v as text)
				else if c is button then
					set n to name of e
					if n is missing value then set n to title of e
					if n is not missing value then
						set end of theNames to (n as text)
						set end of theButtons to (contents of e)
					end if
				end if
			end try
		end repeat
	end tell
	return {theTexts, theNames, theButtons}
end describeWindow

on alertWindows()
	tell application "System Events"
		if not (exists process "UserNotificationCenter") then return {}
		return every window of process "UserNotificationCenter"
	end tell
end alertWindows

on dumpAlerts()
	set outLines to {}
	set i to 0
	repeat with w in my alertWindows()
		set i to i + 1
		set d to my describeWindow(w)
		set end of outLines to "window " & i & ": texts [" & my joined(item 1 of d, " / ") & "] buttons [" & my joined(item 2 of d, ", ") & "]"
	end repeat
	if outLines is {} then return "no UserNotificationCenter window"
	return my joined(outLines, linefeed)
end dumpAlerts

on findAlert(needle, btn)
	repeat with w in my alertWindows()
		set d to my describeWindow(w)
		set t to my joined(item 1 of d, " / ")
		if t contains needle then
			if btn is "" then return t
			set btnNames to item 2 of d
			set btnRefs to item 3 of d
			repeat with k from 1 to count of btnNames
				if item k of btnNames is btn then
					tell application "System Events" to click (item k of btnRefs)
					return "pressed " & btn & " in: " & t
				end if
			end repeat
			error "no button " & btn & " in the window [" & t & "] (buttons: " & my joined(btnNames, ", ") & ")"
		end if
	end repeat
	return ""
end findAlert

on clickMenu(bundleId, prefix)
	tell application "System Events"
		set ps to (every application process whose bundle identifier is bundleId)
		if (count of ps) is 0 then error "no process with the bundle id " & bundleId
		set p to item 1 of ps
		set frontmost of p to true
		delay 1
		set mb to menu bar 1 of p
		set titles to name of every menu bar item of mb
		set theItems to every menu item of menu 1 of menu bar item 2 of mb
		repeat with mi in theItems
			set n to name of mi
			if n is not missing value then
				if (n as text) starts with prefix then
					click mi
					return (n as text) & " (menu bar: " & my joined(titles, ", ") & ")"
				end if
			end if
		end repeat
		error "no item starting with " & prefix & " in the menu " & (name of menu bar item 2 of mb) & " (menu bar: " & my joined(titles, ", ") & ")"
	end tell
end clickMenu
