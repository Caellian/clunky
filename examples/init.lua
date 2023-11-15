function draw_bg(canvas)
    paint = Gfx:newPaint()
    paint:setColor({
        r = 0.1,
        g = 0.6,
        b = 0.8,
        a = 1.0
    })
    canvas:drawCircle(100, 100, 50, paint)
end

settings = {
    framerate = 2,
    background = draw_bg,
}
