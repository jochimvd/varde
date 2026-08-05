hl.monitor({
    output = "",
    mode = "1280x720@60",
    position = "0x0",
    scale = 1,
})

hl.config({
    animations = {
        enabled = false,
    },
    ecosystem = {
        enforce_permissions = true,
    },
    general = {
        border_size = 0,
        gaps_in = 0,
        gaps_out = 0,
    },
    misc = {
        background_color = "rgb(303545)",
        disable_hyprland_logo = true,
        disable_splash_rendering = true,
        disable_watchdog_warning = true,
        force_default_wallpaper = 0,
    },
})

hl.permission({ binary = "/usr/bin/grim", type = "screencopy", mode = "allow" })

for workspace = 1, 5 do
    hl.workspace_rule({ workspace = tostring(workspace), persistent = true })
end

hl.on("hyprland.start", function()
    local shell = os.getenv("SHELL_DEV_BINARY")
    if shell then
        hl.exec_cmd(shell)
    end
end)

hl.bind("CTRL + ALT + Escape", hl.dsp.exit())
