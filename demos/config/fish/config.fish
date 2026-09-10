set -g fish_greeting
set -g fish_autosuggestion_enabled 0

function fish_prompt
    set_color a6e3a1
    printf '❯ '
    set_color normal
end
