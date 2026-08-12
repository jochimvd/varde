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

hl.on("hyprland.start", function()
    local varde = os.getenv("VARDE_DEV_BINARY")
    if varde then
        hl.exec_cmd(varde .. " start")
    end
end)

hl.bind("CTRL + ALT + Escape", hl.dsp.exit())
hl.bind("CTRL + Q", hl.dsp.window.close())
local varde = os.getenv("VARDE_DEV_BINARY") or "varde"
hl.bind("CTRL + SPACE", hl.dsp.exec_cmd(varde .. " launcher"))
hl.bind("CTRL + V", hl.dsp.exec_cmd(varde .. " clipboard"))

for workspace = 1, 9 do
    hl.bind("CTRL + " .. workspace, hl.dsp.focus({ workspace = workspace }))
end
