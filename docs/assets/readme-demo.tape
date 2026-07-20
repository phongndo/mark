Require ./target/release/mark
Require git

Set Shell "bash"
Set FontFamily "Menlo"
Set FontSize 18
Set Width 1600
Set Height 1000
Set Padding 22
Set Margin 42
Set MarginFill "#0b0d17"
Set BorderRadius 16
Set WindowBar "Rings"
Set WindowBarSize 38
Set Theme "Catppuccin Mocha"
Set TypingSpeed 18ms

Hide
Type "export PS1='❯ '"
Enter
Type "project_root=$PWD"
Enter
Type "export PATH=$project_root/target/release:$PATH"
Enter
Type "export XDG_CONFIG_HOME=$project_root/docs/assets/demo-config"
Enter
Type "unset NO_COLOR"
Enter
Type "git show --format= 5b2e789 -- crates/mark-tui/src/editor.rs > target/readme-demo.patch"
Enter
Type "clear"
Enter
Show

Type "mark patch target/readme-demo.patch"
Sleep 250ms
Enter
Sleep 1.2s

Tab
Sleep 300ms
Tab
Sleep 300ms
Type "]"
Sleep 500ms

Screenshot docs/assets/mark-demo.png

Type "q"
