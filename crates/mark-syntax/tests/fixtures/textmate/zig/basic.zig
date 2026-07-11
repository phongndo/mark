// Zig basic with non-ASCII café
const std = @import("std");
pub fn main() !void {
    std.debug.print("héllo {d}\n", .{42});
}
