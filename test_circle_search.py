from circle_search import BTN_H, adjust, button_rects, hit, norm

W, H = 1920, 1080

# rectangle from two corners in any drag direction, clamped to the screen
assert norm(300, 200, 100, 50, W, H) == (100, 50, 200, 150)
assert norm(-20, -5, 2000, 1100, W, H) == (0, 0, W, H)

# corners, edges, inside, outside
sel = (100, 100, 200, 100)
assert hit(sel, 100, 100) == "nw" and hit(sel, 300, 200) == "se"
assert hit(sel, 200, 101) == "n" and hit(sel, 299, 150) == "e"
assert hit(sel, 200, 150) == "move" and hit(sel, 500, 500) is None

# resizing a corner, flipping past the opposite edge, and moving within the screen
assert adjust("se", sel, 50, 20, W, H) == (100, 100, 250, 120)
assert adjust("w", sel, 250, 0, W, H) == (300, 100, 50, 100)
assert adjust("move", sel, 5000, -5000, W, H) == (W - 200, 0, 200, 100)

# buttons go below the selection when there's room, above it near the bottom edge
assert button_rects((100, 100, 300, 200), W, H)[0][1] == 312
assert button_rects((100, 900, 300, 170), W, H)[0][1] == 900 - 12 - BTN_H
# full-screen selection: buttons sit inside, and never run off the right edge
rects = button_rects((0, 0, W, H), W, H)
assert rects[0][1] == H - 12 - BTN_H and rects[-1][0] + rects[-1][2] <= W
assert button_rects((1800, 100, 100, 100), W, H)[-1][0] + 150 <= W
print("ok")
