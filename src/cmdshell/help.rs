//! `HELP command` / `command /?` texts. Laid out like cmd.exe's (one-line summary, syntax, switches),
//! written for Aerode: they describe what this interpreter actually does on Linux.

/// Usage text of a built-in command (canonical name, upper case), or None.
pub fn usage(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "ATTRIB" => "\
Shows or changes file attributes.

ATTRIB [+R | -R] [[drive:][path]filename]

  +R   Makes the file read-only (removes write permission).
  -R   Makes the file writable again.

Files whose names start with a dot are shown as hidden (H).",
        "CALL" => "\
Runs a batch file or a labelled part of the current batch file, then continues.

CALL [drive:][path]filename [parameters]
CALL :label [parameters]

  :label   Runs the batch file from that label up to its end or EXIT /B.",
        "CD" => "\
Shows the name of the current folder or changes to another one.

CHDIR [/D] [drive:][path]
CHDIR [..]
CD [/D] [drive:][path]
CD [..]

  ..   Goes up to the parent folder.
  /D   Accepted for compatibility (there is only drive C:).

CD without a path prints the current folder. Folder names are matched without
regard to case, and C:\\Users stands for /home.",
        "CLS" => "\
Clears the screen and the scroll-back buffer.

CLS",
        "COLOR" => "\
Sets the default text and background colours of the console.

COLOR [attr]

  attr   Two hex digits: the first is the background, the second the text.

    0 = Black       8 = Gray
    1 = Blue        9 = Light Blue
    2 = Green       A = Light Green
    3 = Aqua        B = Light Aqua
    4 = Red         C = Light Red
    5 = Purple      D = Light Purple
    6 = Yellow      E = Light Yellow
    7 = White       F = Bright White

COLOR without a value restores the colours from the Properties dialog.
Both digits may not be the same; ERRORLEVEL is then set to 1.
Example: \"COLOR 0A\" gives light green text on black.",
        "COPY" => "\
Copies one or more files to another place.

COPY [/Y | /-Y] source [+ source ...] [destination]

  source        File(s) to copy; wildcards are allowed.
  destination   Folder or file name for the copies (default: current folder).
  /Y            Overwrites existing files without asking.
  /-Y           Asks before overwriting.

The number of files copied is shown when done.",
        "DATE" => "\
Shows the current date.

DATE [/T]

  /T   Only prints the date, without asking for a new one.

Changing the date needs administrator rights; use the Date and Time
settings in Control Panel instead.",
        "DEL" => "\
Deletes one or more files.

DEL [/P] [/F] [/S] [/Q] names
ERASE [/P] [/F] [/S] [/Q] names

  names   Files or folders (all files in a folder); wildcards are allowed.
  /P      Asks before deleting each file.
  /F      Also deletes read-only files.
  /S      Deletes matching files in all subfolders too.
  /Q      Quiet: does not ask before deleting *.* or a whole folder.

Deleted files do not go to the Recycle Bin.",
        "DIR" => "\
Lists the files and subfolders of a folder.

DIR [drive:][path][filename] [/A] [/B] [/O[[:]order]] [/S] [/W]

  /A      Also shows hidden files (names starting with a dot).
  /B      Bare format: names only, no heading or summary.
  /O      Sort order:  N  by name     S  by size     D  by date/time
                       G  folders first              -  reverse order
  /S      Includes all subfolders.
  /W      Wide list format.

The listing ends with the number of files, their total size and the free
space on the drive.",
        "ECHO" => "\
Prints a message, or turns command echoing on or off.

  ECHO [ON | OFF]
  ECHO [message]
  ECHO.

ECHO without a parameter shows whether echoing is on. ECHO. prints an empty
line. In a batch file, @ in front of a line hides just that line.",
        "ENDLOCAL" => "\
Ends the local environment started by SETLOCAL in a batch file and restores
the variables and current folder from before it.

ENDLOCAL",
        "EXIT" => "\
Closes the command processor (and its window), or ends a batch file.

EXIT [/B] [exitCode]

  /B         Leaves only the current batch file (or CALLed label).
  exitCode   Number to return: the window's exit code, or ERRORLEVEL with /B.",
        "FIND" => "\
Searches for a text string in files or in the output of another command.

FIND [/V] [/C] [/I] \"string\" [[drive:][path]filename [...]]

  /V   Shows the lines that do NOT contain the string.
  /C   Shows only the number of matching lines.
  /I   Ignores the case of letters.

Example: DIR | FIND /I \"txt\"",
        "FOR" => "\
Runs a command once for every item of a set.

FOR %variable IN (set) DO command [parameters]
FOR /D %variable IN (set) DO command       (matches folders only)
FOR /R [path] %variable IN (set) DO command  (in every folder of the tree)
FOR /L %variable IN (start,step,end) DO command
FOR /F [\"options\"] %variable IN (file | \"string\" | 'command') DO command

  %variable   A single letter. Write it as %%x inside batch files.
  (set)       Words or wildcard patterns such as *.txt.
  /L          Counts from start to end in steps of step.
  /F          Takes lines from a file, a quoted string or the output of a
              command and splits them into tokens. Options:
                tokens=1,3 or 2-4 or 2*  which tokens; each goes into the
                                         next letter (%a, %b, ...), * = rest
                delims=xy                separators (default: blank and tab)
                skip=n                   ignore the first n lines
                eol=c                    ignore lines starting with c
                usebackq                 `command` and 'string' quoting

%~nf, %~xf, %~dpf, %~ff, %~zf: the name, extension, folder, full path or
size of the item, as for batch file parameters.

Example: FOR %f IN (*.log) DO TYPE %f",
        "GOTO" => "\
Continues a batch file at the line after a label.

GOTO label

  label   Written in the batch file as :label on a line of its own.

GOTO :EOF ends the batch file (or the CALLed label).",
        "HELP" => "\
Shows help for the commands of this command processor.

HELP [command]

  command   Shows the details of one command (the same as command /?).",
        "IF" => "\
Runs a command only when a condition is true (batch files and prompt).

IF [NOT] ERRORLEVEL number command
IF [/I] [NOT] string1==string2 command
IF [NOT] EXIST filename command
IF [NOT] DEFINED variable command
IF [/I] string1 compare-op string2 command

  NOT          Runs the command when the condition is false.
  ERRORLEVEL   True when the last exit code is number or higher.
  EXIST        True when the file or folder exists (wildcards allowed).
  DEFINED      True when the environment variable is set.
  /I           Compares strings without regard to case.
  compare-op   EQU NEQ LSS LEQ GTR GEQ; numbers are compared as numbers.

An ELSE part can follow:  IF EXIST a.txt (ECHO yes) ELSE (ECHO no)",
        "MD" => "\
Creates a folder, including any missing parent folders.

MKDIR [drive:]path
MD [drive:]path",
        "MKLINK" => "\
Creates a symbolic link (or a hard link).

MKLINK [[/D] | [/H] | [/J]] Link Target

  /D   Directory link (a symbolic link also works for folders on Linux).
  /H   Hard link instead of a symbolic link.
  /J   Treated like /D.",
        "MORE" => "\
Shows output or a file one screenful at a time.

command | MORE
MORE [drive:][path]filename

Press any key to see the next screen.",
        "MOVE" => "\
Moves files, or renames a file or folder.

MOVE [/Y | /-Y] [drive:][path]filename1[,...] destination
MOVE [/Y | /-Y] [drive:][path]dirname1 dirname2

  /Y    Overwrites existing files without asking.
  /-Y   Asks before overwriting.",
        "PATH" => "\
Shows or sets the folders searched for programs.

PATH [[drive:]path[;...]]
PATH ;

PATH without a parameter shows the current search path.
PATH ; clears it.",
        "PAUSE" => "\
Stops a batch file and shows \"Press any key to continue . . .\"

PAUSE",
        "POPD" => "\
Goes back to the folder saved by the last PUSHD.

POPD",
        "PROMPT" => "\
Changes the command prompt.

PROMPT [text]

  text   Plain text plus these codes:
    $P  current drive and path     $G  >      $L  <      $B  |
    $Q  =      $S  space           $_  new line          $$  $
    $D  date   $T  time            $V  version           $N  drive
    $A  &      $C  (    $F  )      $E  escape character
    $+  one + for each folder saved with PUSHD

PROMPT without text restores $P$G.",
        "PUSHD" => "\
Saves the current folder for POPD, then changes to another one.

PUSHD [path]

PUSHD without a path lists the saved folders.",
        "RD" => "\
Removes (deletes) a folder.

RMDIR [/S] [/Q] [drive:]path
RD [/S] [/Q] [drive:]path

  /S   Also removes every file and folder inside it.
  /Q   Quiet: does not ask for confirmation with /S.

Without /S only an empty folder can be removed.",
        "REM" => "\
Marks a comment in a batch file. The rest of the line is ignored.

REM [comment]",
        "REN" => "\
Renames a file or folder.

RENAME [drive:][path]name1 name2
REN [drive:][path]name1 name2

The new name cannot contain a path: the file stays in its folder.",
        "SET" => "\
Shows, sets or removes environment variables.

SET [variable=[string]]
SET /A expression
SET /P variable=[promptString]

  variable=string   Sets the variable; with nothing after = it is removed.
  /A                Calculates an integer expression:
                      ( )  * / %  + -  << >>  &  ^  |  and = *= /= %= += -=
  /P                Asks the user for the value, showing promptString.

SET alone lists all variables; SET P lists those whose names start with P.
Use %name% to insert a value and %name:old=new% to replace text in it.",
        "SETLOCAL" => "\
Starts a local environment in a batch file: changes to variables and to the
current folder are undone by ENDLOCAL or at the end of the batch file.

SETLOCAL",
        "SHIFT" => "\
Shifts the parameters of a batch file: %2 becomes %1, %3 becomes %2...

SHIFT",
        "SORT" => "\
Sorts lines of text from a file or from another command.

SORT [/R] [[drive:][path]filename]

  /R   Reverses the order (Z to A).

Lines are compared without regard to case.",
        "START" => "\
Opens a program, document, folder or web address in its own window.

START [\"title\"] [/D path] [/WAIT] [command/program] [parameters]

  \"title\"   Title for a new Command Prompt window.
  /D path    Folder the program starts in.
  /WAIT      Waits until the program ends.

START alone opens a new Command Prompt window. Documents open with their
default program, folders in Windows Explorer.",
        "TIME" => "\
Shows the current time.

TIME [/T]

  /T   Only prints the time, without asking for a new one.",
        "TIMEOUT" => "\
Waits a number of seconds, or until a key is pressed.

TIMEOUT [/T] seconds [/NOBREAK]

  /NOBREAK   Ignores key presses.",
        "TITLE" => "\
Sets the title of the Command Prompt window.

TITLE [string]",
        "TREE" => "\
Draws the folder structure of a path as a tree.

TREE [drive:][path] [/F] [/A]

  /F   Also lists the files in each folder.
  /A   Uses plain ASCII instead of line-drawing characters.",
        "TYPE" => "\
Shows the contents of one or more text files.

TYPE [drive:][path]filename",
        "VER" => "\
Shows the version of the command processor.

VER",
        "VOL" => "\
Shows the label and serial number of the drive.

VOL [drive:]",
        "WHERE" => "\
Shows where a program is, searching the folders of PATH.

WHERE pattern",
        "TASKLIST" => "\
Lists the running processes with their process ID and memory use.

TASKLIST",
        "TASKKILL" => "\
Ends one or more processes.

TASKKILL [/F] { /PID processid | /IM imagename }

  /F    Forces the process to end (SIGKILL instead of SIGTERM).
  /PID  Process ID, as shown by TASKLIST.
  /IM   Program name, for example /IM gedit.",
        "IPCONFIG" => "\
Shows the IP configuration of the network adapters.

IPCONFIG [/ALL]

  /ALL   Also shows the host name and the physical (MAC) addresses.",
        "PING" => "\
Checks whether another computer answers over the network.

PING [-n count] target_name

  -n count   Number of echo requests to send (default 4).",
        "CLIP" => "\
Puts the output of another command on the clipboard.

command | CLIP",
        "WHOAMI" => "\
Shows the computer and user name of the current user.

WHOAMI",
        "HOSTNAME" => "\
Shows the name of this computer.

HOSTNAME",
        "SYSTEMINFO" => "\
Shows basic information about this computer and its operating system.

SYSTEMINFO",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_listed_command_has_help() {
        for (name, _) in super::super::BUILTINS {
            let canon = match *name { "CHDIR" => "CD", "ERASE" => "DEL", "MKDIR" => "MD", "RMDIR" => "RD", "RENAME" => "REN", n => n };
            let u = super::usage(canon).unwrap_or_else(|| panic!("no help for {name}"));
            // fits an 80-column console
            assert!(u.lines().all(|l| l.chars().count() <= 79), "{name}");
        }
    }
}
