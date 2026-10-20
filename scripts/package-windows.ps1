# Build what to hand to somebody: Voi-<version>.<build>.exe, which installs Voi
# (scripts\voi.iss, made with Inno Setup 6), and the same files as a zip to
# run without installing: Voi.exe, the command line, a read-me. Nothing else,
# and no key. Without Inno Setup only the zip is made.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
#
# Clean inputs only. Inside a git checkout the sources are exported from the
# commit (`git archive HEAD`), so an untracked .env, a config file or a session
# lying in the working tree cannot reach the build; outside one, only the
# source folders are copied. The build gets a target folder of its own, and the
# zip is assembled from a fixed list of files rather than by copying a folder.
#
# Then the files are scanned — every file, every string in every binary — for
# the Soniox key this machine uses (read from SONIOX_API_KEY or the user's Voi
# config, now, never stored) and for anything shaped like one: the folder the
# setup is made from (its insides are compressed), the setup, and the zip. On
# a hit nothing is kept and the script fails: there is nothing to share.
#
# Signing is ready for when there is a code-signing certificate, and off until
# then (Windows then warns once, "Windows protected your PC", on a computer the
# setup was downloaded to). Give one of:
#   VOI_SIGN_THUMBPRINT   a certificate in your or the machine's store - a
#                         hardware token, an imported .pfx, a cloud signer's
#                         client. Preferred: no password on any command line.
#   VOI_SIGN_PFX          a .pfx file, with VOI_SIGN_PASSWORD. The password goes
#                         to signtool on its command line; it is never printed.
# and optionally VOI_SIGN_TIMESTAMP (an RFC 3161 server, default DigiCert's)
# and SIGNTOOL (signtool.exe, otherwise the newest Windows SDK's). Then
# Voi.exe, cli\voi.exe, the setup and its uninstaller are all signed.
param(
    [string]$Out = ""
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# signtool's arguments for the certificate given, or none: unsigned.
function Get-SignArgs {
    $by = if ($env:VOI_SIGN_THUMBPRINT) {
        @('/sha1', ($env:VOI_SIGN_THUMBPRINT -replace '\s', ''))
    } elseif ($env:VOI_SIGN_PFX) {
        if (-not (Test-Path $env:VOI_SIGN_PFX)) { throw "VOI_SIGN_PFX: no such file" }
        @('/f', $env:VOI_SIGN_PFX) + $(if ($env:VOI_SIGN_PASSWORD) { @('/p', $env:VOI_SIGN_PASSWORD) } else { @() })
    } else {
        return $null
    }
    $stamp = if ($env:VOI_SIGN_TIMESTAMP) { $env:VOI_SIGN_TIMESTAMP } else { 'http://timestamp.digicert.com' }
    @('/fd', 'sha256', '/tr', $stamp, '/td', 'sha256') + $by
}

function Get-SignTool {
    if ($env:SIGNTOOL) { return $env:SIGNTOOL }
    $kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    $found = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue | Sort-Object { try { [version]$_.Name } catch { [version]'0.0' } } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $found) { throw "signing asked for, but no signtool.exe: install the Windows SDK or set SIGNTOOL" }
    $found
}

function Invoke-Sign([string]$Path) {
    & $script:SignTool sign /q @script:SignArgs $Path | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "signing $(Split-Path -Leaf $Path) failed" }
    Write-Host "  signed $(Split-Path -Leaf $Path): $((Get-AuthenticodeSignature $Path).SignerCertificate.Subject)"
}

$SignArgs = Get-SignArgs
$SignTool = if ($SignArgs) { Get-SignTool } else { $null }
if (-not $Out) { $Out = Join-Path $Root 'target\Voi-windows-x64.zip' }
$Work = Join-Path ([IO.Path]::GetTempPath()) ("voi-package-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
$Src = Join-Path $Work 'src'
$Stage = Join-Path $Work 'Voi'
New-Item -ItemType Directory -Force $Src, $Stage, (Join-Path $Stage 'cli') | Out-Null

try {
    Write-Host "> sources"
    $git = Get-Command git -ErrorAction SilentlyContinue
    $inRepo = $false
    if ($git) {
        # Windows PowerShell turns git's "not a git repository" into a
        # terminating error under 'Stop'; here it only means "no".
        $ErrorActionPreference = 'Continue'
        $inRepo = (& git -C $Root rev-parse --is-inside-work-tree 2>$null) -eq 'true'
        $ErrorActionPreference = 'Stop'
    }
    if ($inRepo) {
        if (& git -C $Root status --porcelain) {
            Write-Warning "uncommitted changes are not in the package: it is built from HEAD"
        }
        $tar = Join-Path $Work 'src.tar'
        & git -C $Root archive --format=tar -o $tar HEAD
        tar -xf $tar -C $Src
        Remove-Item $tar
        $Commit = (& git -C $Root rev-parse --short HEAD)
        # Commits since config/build-base, as bundle.sh counts them.
        $Build = [int](& git -C $Root rev-list --count HEAD) + [int]((Get-Content (Join-Path $Root 'config\build-base')).Trim())
    } else {
        foreach ($item in 'Cargo.toml', 'Cargo.lock', 'crates', 'apps', 'assets', 'config') {
            Copy-Item -Recurse (Join-Path $Root $item) $Src
        }
        New-Item -ItemType Directory -Force (Join-Path $Src 'scripts') | Out-Null
        Copy-Item (Join-Path $Root 'scripts\voi.iss') (Join-Path $Src 'scripts')
        # A copy without its history says which commit it is through VOI_COMMIT.
        $Commit = if ($env:VOI_COMMIT) { $env:VOI_COMMIT } else { 'not a git checkout' }
        $Build = if ($env:VOI_BUILD) { $env:VOI_BUILD } else { '0' }
    }
    # The commit count, as on the Mac: what the app compares with the latest
    # release to offer an update. 0, unknown, never asks.

    Write-Host "> building (release)"
    # The build never reads the key; keep it out of the build's environment
    # all the same, and back for the scan.
    $key = $env:SONIOX_API_KEY
    $env:SONIOX_API_KEY = $null
    $env:CARGO_TARGET_DIR = Join-Path $Work 'target'
    $voiBuild = $env:VOI_BUILD
    $env:VOI_BUILD = $Build
    # The compiler writes the paths it built from into the binary — the
    # user's folder, and so the user's name, among them. The package names
    # neutral ones instead. (The last match wins: wider paths first.)
    $remap = @("$env:USERPROFILE=C:\home", "$env:USERPROFILE\.cargo=C:\cargo", "$env:USERPROFILE\.rustup=C:\rustup", "$Work=C:\build", "$Src=C:\voi") |
        ForEach-Object { "--remap-path-prefix=$_" }
    $rustflags = $env:RUSTFLAGS
    $env:RUSTFLAGS = (@($env:RUSTFLAGS) + $remap | Where-Object { $_ }) -join ' '
    Push-Location $Src
    try {
        & cargo build --release --locked -p voi-app -p voi-cli -p voi-scan
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally {
        Pop-Location
        $env:SONIOX_API_KEY = $key
        $env:RUSTFLAGS = $rustflags
        $env:VOI_BUILD = $voiBuild
        Remove-Item Env:CARGO_TARGET_DIR
    }
    $bin = Join-Path $Work 'target\release'

    Write-Host "> assembling"
    # Voi.exe and voi.exe would be one file — NTFS ignores case — so the
    # command line goes in a folder of its own.
    Copy-Item (Join-Path $bin 'voi-app.exe') (Join-Path $Stage 'Voi.exe')
    Copy-Item (Join-Path $bin 'voi.exe') (Join-Path $Stage 'cli\voi.exe')
    if ($SignArgs) {
        Write-Host "> signing"
        Invoke-Sign (Join-Path $Stage 'Voi.exe')
        Invoke-Sign (Join-Path $Stage 'cli\voi.exe')
    }
    $version = (Select-String -Path (Join-Path $Src 'Cargo.toml') -Pattern '^version = "(.*)"').Matches[0].Groups[1].Value
    # The setup carries the build too, as the release's files are named.
    $full = if ($Build -ne '0') { "$version.$Build" } else { $version }
    @"
Voi $version for Windows ($Commit)

Start Voi.exe (or install it with Voi-$full.exe, which puts it in the Start
menu and can start it when you sign in). Its icon - a dot, red while recording - goes in the
notification area: on Windows 11 behind the ^ next to the clock until you drag
it onto the taskbar or switch Voi on in Settings > Personalization > Taskbar.
Click it for the menu. The HUD shows in the top-right corner while you record.

Paste your Soniox API key into Settings (double-click the tray icon). It is
kept in %APPDATA%\voi\config.env, readable only by you, never in this folder.

Hold Right Ctrl, speak, let go: it is typed where the cursor is. The key, and
where the dictation window appears, are in Settings > Dictation.
Transcripts: %APPDATA%\voi\sessions, one Markdown file per session.

cli\voi.exe is the command line: voi listen, voi transcribe FILE, voi sessions.
"@ | Set-Content -Encoding UTF8 (Join-Path $Stage 'README.txt')

    $scan = Join-Path $bin 'voi-scan.exe'
    Write-Host "> scanning the files for a key"
    & $scan $Stage
    if ($LASTEXITCODE -ne 0) { throw "a key, or something shaped like one, is in the files - nothing built" }

    $OutDir = Split-Path $Out
    New-Item -ItemType Directory -Force $OutDir | Out-Null
    if (Test-Path $Out) { Remove-Item $Out }
    Compress-Archive -Path $Stage -DestinationPath $Out
    $made = @($Out)

    $iscc = @($env:ISCC, "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
        Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if ($iscc) {
        Write-Host "> setup"
        $setup = Join-Path $OutDir "Voi-$full.exe"
        if (Test-Path $setup) { Remove-Item $setup }
        $isccArgs = @('/Q', "/DAppVersion=$full", "/DSource=$Stage", "/DOutputDir=$OutDir")
        if ($SignArgs) {
            # Inno Setup signs the setup and the uninstaller it writes at
            # install time; `$f` is the file, `$q` a quote.
            $quoted = ($SignArgs | ForEach-Object { if ($_ -match '\s') { '$q' + $_ + '$q' } else { $_ } }) -join ' '
            $isccArgs += '/DSign=1', ('/Svoisign=$q' + $SignTool + '$q sign /q ' + $quoted + ' $f')
        }
        & $iscc @isccArgs (Join-Path $Src 'scripts\voi.iss')
        if ($LASTEXITCODE -ne 0) { throw "the setup could not be built" }
        if ($SignArgs) { Write-Host "  signed $(Split-Path -Leaf $setup): $((Get-AuthenticodeSignature $setup).SignerCertificate.Subject)" }
        $made += $setup
    } else {
        Write-Warning "Inno Setup 6 not found (set ISCC to its ISCC.exe): the zip only, no setup"
    }

    Write-Host "> scanning what is to be shared"
    & $scan @made
    if ($LASTEXITCODE -ne 0) {
        $made | ForEach-Object { Remove-Item $_ -ErrorAction SilentlyContinue }
        throw "a key, or something shaped like one, is in the package - deleted"
    }

    Write-Host ""
    foreach ($file in $made) {
        $hash = (Get-FileHash -Algorithm SHA256 $file).Hash.ToLower()
        $size = [math]::Round((Get-Item $file).Length / 1MB, 1)
        Write-Host "Built $file ($size MB) - Voi $version, $Commit"
        Write-Host "  sha256 $hash"
    }
    if (-not $SignArgs) { Write-Host "Not signed: no VOI_SIGN_THUMBPRINT or VOI_SIGN_PFX given." }
    Write-Host "Share these and nothing else: the setup to install Voi, the zip to run it without installing."
} finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
