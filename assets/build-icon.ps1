# Regenerates icon.ico and icon-256.png from icon.svg. Requires ImageMagick 7 (`magick`) on PATH.
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$svg = Join-Path $here 'icon.svg'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('image-moz-icon-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $sizes = 16, 20, 24, 32, 40, 48, 64, 256
    foreach ($s in $sizes) {
        # Rasterize at high density, then downscale for clean edges.
        & magick -background none -density 1536 $svg -resize "${s}x${s}" "PNG32:$tmp\$s.png"
        if ($LASTEXITCODE -ne 0) { throw "magick failed for size $s" }
    }
    $pngs = $sizes | ForEach-Object { "$tmp\$_.png" }
    & magick @pngs (Join-Path $here 'icon.ico')
    if ($LASTEXITCODE -ne 0) { throw 'magick failed building icon.ico' }
    Copy-Item -Force "$tmp\256.png" (Join-Path $here 'icon-256.png')
} finally {
    Remove-Item -Recurse -Force $tmp
}
Write-Host 'Wrote icon.ico and icon-256.png'
