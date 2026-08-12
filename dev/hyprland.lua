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
        no_donation_nag = true,
        no_update_news = true,
    },
    general = {
        border_size = 2,
        gaps_in = 4,
        gaps_out = 8,
        col = {
            active_border = "rgba(FFFFFFee)",
            inactive_border = "rgba(595959ee)",
        },
    },
    decoration = {
        rounding = 5,
        shadow = {
            enabled = false,
        },
    },
    misc = {
        background_color = "rgb(303545)",
        disable_hyprland_logo = true,
        disable_splash_rendering = true,
        disable_watchdog_warning = true,
        focus_on_activate = true,
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
