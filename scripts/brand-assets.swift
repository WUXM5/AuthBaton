import AppKit
import SwiftUI

// Only presentation, resampling and macOS container/format generation.
// The logo geometry is supplied by the reviewed source PNG.
let arguments=CommandLine.arguments
guard arguments.count==4, let mark=NSImage(contentsOfFile:arguments[2]) else {
    fatalError("usage: brand-assets <preview|pack> <mark.png> <output path>")
}
func bitmap(_ width:Int,_ height:Int,_ draw:()->Void)->NSBitmapImageRep {
    let rep=NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:width,pixelsHigh:height,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:width*4,bitsPerPixel:32)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current=NSGraphicsContext(bitmapImageRep:rep)
    NSGraphicsContext.current?.imageInterpolation = .high
    NSColor.clear.setFill();NSRect(x:0,y:0,width:width,height:height).fill(using:.copy)
    draw()
    NSGraphicsContext.restoreGraphicsState()
    return rep
}
func save(_ rep:NSBitmapImageRep,_ path:String)throws {
    try rep.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:path))
}
func image(_ rep:NSBitmapImageRep)->NSImage {
    let value=NSImage(size:NSSize(width:rep.pixelsWide,height:rep.pixelsHigh));value.addRepresentation(rep);return value
}
// On the 无垠青 tile the mark is white with its crossing at half strength, as the app draws it;
// the master paints the crossing opaque grey (171) and the loop near-white (243).
let tileMark:NSImage = {
    let source=NSBitmapImageRep(data:mark.tiffRepresentation!)!
    let rep=bitmap(source.pixelsWide,source.pixelsHigh){source.draw(in:NSRect(x:0,y:0,width:source.pixelsWide,height:source.pixelsHigh))}
    var pixel=[Int](repeating:0,count:4)
    for y in 0..<rep.pixelsHigh { for x in 0..<rep.pixelsWide {
        rep.getPixel(&pixel,atX:x,y:y)
        guard pixel[3]>0 else {continue}
        let light=Double(pixel[0])*255/Double(pixel[3])
        let alpha=Int((Double(pixel[3])*min(1,max(0.5,0.5+(light-171)/(243-171)*0.5))).rounded())
        var white=[alpha,alpha,alpha,alpha]
        rep.setPixel(&white,atX:x,y:y)
    } }
    return image(rep)
}()
// The macOS icon grid: an 824 tile with 185.4 continuous corners centred on the 1024 canvas,
// under the drop shadow macOS draws for other apps' icons.
func icon(_ size:Int)->NSBitmapImageRep {
    bitmap(size,size) {
        let scale=CGFloat(size)/1024
        let t=NSAffineTransform();t.scale(by:scale);t.concat()
        let frame=NSRect(x:100,y:100,width:824,height:824)
        let tile=NSBezierPath(cgPath:RoundedRectangle(cornerRadius:185.4,style:.continuous).path(in:frame).cgPath)
        NSGraphicsContext.saveGraphicsState()
        // Shadow geometry is in device pixels, outside the scale transform.
        let shadow=NSShadow();shadow.shadowColor=NSColor(white:0,alpha:0.25)
        shadow.shadowOffset=NSSize(width:0,height:-8*scale);shadow.shadowBlurRadius=31*scale;shadow.set()
        NSColor(srgbRed:8/255,green:151/255,blue:184/255,alpha:1).setFill();tile.fill()
        NSGraphicsContext.restoreGraphicsState()
        NSColor(white:0,alpha:0.12).setStroke();tile.lineWidth=1.5;tile.stroke()
        // The mark keeps its share of the tile: 832 of 928, inset 48.
        tileMark.draw(in:frame.insetBy(dx:824*48/928,dy:824*48/928))
    }
}
func label(_ text:String,_ x:CGFloat,_ y:CGFloat,_ size:CGFloat=14,_ white:CGFloat=0.68) {
    (text as NSString).draw(at:NSPoint(x:x,y:y),withAttributes:[.font:NSFont.systemFont(ofSize:size,weight:.medium),.foregroundColor:NSColor(calibratedWhite:white,alpha:1)])
}
if arguments[1]=="preview" {
    let preview=bitmap(1100,720) {
        NSColor(calibratedWhite:0.065,alpha:1).setFill();NSRect(x:0,y:0,width:1100,height:720).fill()
        label("AUTHMANAGER / MARK STUDY",48,655,16,0.86)
        mark.draw(in:NSRect(x:28,y:108,width:570,height:570))
        label("Transparent master on charcoal",72,84)
        image(icon(256)).draw(in:NSRect(x:660,y:376,width:256,height:256))
        label("Dock / 256 px",680,350)
        let small=bitmap(18,18) {mark.draw(in:NSRect(x:0,y:0,width:18,height:18));NSColor.white.setFill();NSRect(x:0,y:0,width:18,height:18).fill(using:.sourceIn)}
        image(small).draw(in:NSRect(x:673,y:270,width:18,height:18))
        label("Menu / 18 px",711,270)
        let sidebar=bitmap(42,42) {mark.draw(in:NSRect(x:0,y:0,width:42,height:42))}
        image(sidebar).draw(in:NSRect(x:673,y:190,width:42,height:42))
        label("Sidebar / 42 px",735,202)
        NSGraphicsContext.current?.imageInterpolation = .none
        image(small).draw(in:NSRect(x:675,y:43,width:108,height:108))
        label("18 px / 6× detail",816,90,13)
    }
    try save(preview,arguments[3])
} else if arguments[1]=="pack" {
    let output=arguments[3]
    try FileManager.default.createDirectory(atPath:output,withIntermediateDirectories:true)
    let iconset=output+"/AppIcon.iconset"
    try FileManager.default.createDirectory(atPath:iconset,withIntermediateDirectories:true)
    for size in [16,32,128,256,512] {
        try save(icon(size),"\(iconset)/icon_\(size)x\(size).png")
        try save(icon(size*2),"\(iconset)/icon_\(size)x\(size)@2x.png")
    }
    try save(bitmap(512,512){mark.draw(in:NSRect(x:0,y:0,width:512,height:512))},output+"/BrandMark.png")
    let menu=bitmap(36,36){mark.draw(in:NSRect(x:0,y:0,width:36,height:36));NSColor.black.setFill();NSRect(x:0,y:0,width:36,height:36).fill(using:.sourceIn)}
    try save(menu,output+"/MenuMark.png")
} else {fatalError("unknown mode")}
