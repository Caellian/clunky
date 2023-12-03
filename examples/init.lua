font_tf = Typeface:MakeFromName("Courier New")
font = Gfx:newFont(font_tf)

primary = {
    h = 120,
    s = 0.5,
    l = 0.6,
    anti_alias = true,
    style = {
        stroke = true,
    },
}

function cpu_arc(canvas, position, radius, cpu_info)
    local core_count = 6

    local angleIncrement = 360.0 / core_count

    for i = 1, core_count do
        local startAngle = (i - 0.5) * angleIncrement
        local endAngle = (i + 0.5) * angleIncrement - angleIncrement / 2

        local path = Gfx:newPath()
        path:addArc({
            left = position[1] - radius,
            top = position[2] - radius,
            right = position[1] + radius,
            bottom = position[2] + radius
        }, startAngle, endAngle - startAngle)

        canvas:drawPath(path, primary)
    end
end

function render(canvas, state)
    cpu_arc(
        canvas,
        { canvas:width() / 2, canvas:height() / 2 },
        canvas:height() / 4,
        state["system"]
    )
end

settings = {
    draw = render,
    collectors = {
        username = "caellian",
    }
}
