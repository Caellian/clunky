font_tf = Gfx:newTypeface("Courier New")
font = Gfx:newFont(font_tf)

function render(canvas, state)
    local test_string = Gfx:newTextBlob(state["test_collector"], font)
    
    canvas:drawTextBlob(test_string, math.random(10, 50), math.random(10, 50), {
        h = 20,
        s = 0.5,
        l = 0.6
    })
end

function test(status)
    status:requestUpdate(200)

    local handle = io.popen('date +"%T.%N"')
    local result = handle:read("*a")
    handle:close()
    return result
end

settings = {
    draw = render,
    collectors = {
        ["username"] = "caellian",
        ["test_collector"] = test,
    }
}
