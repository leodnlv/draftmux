# Installs the latest Draftmux release for this user on x64 Windows 10 1803
# or later:
#
#   irm https://raw.githubusercontent.com/raine/draftmux/main/install.ps1 | iex
#
# The app tree goes in %LOCALAPPDATA%\Programs\Draftmux, its bin folder is
# added to the user's PATH for the draftmux command, and a Start Menu
# shortcut starts the app. Running it again updates Draftmux.
#
# DRAFTMUX_VERSION picks a release instead of the latest, and
# DRAFTMUX_INSTALL_DIR replaces the install folder.

# Everything runs inside the function, so a failure throws instead of
# closing the PowerShell window that piped the script into iex, and nothing
# runs until the whole script has downloaded.
function Install-Draftmux {
    $ErrorActionPreference = 'Stop'
    # Windows PowerShell's progress bar slows downloads severalfold.
    $ProgressPreference = 'SilentlyContinue'
    # Windows PowerShell may not offer TLS 1.2 by default, which GitHub needs.
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $releases = if ($env:DRAFTMUX_RELEASES_URL) { $env:DRAFTMUX_RELEASES_URL } else {
        'https://github.com/raine/draftmux/releases'
    }
    function Fail([string]$message) { throw "draftmux install: $message" }

    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
        Fail "Draftmux for Windows needs x64 Windows, not $env:PROCESSOR_ARCHITECTURE"
    }
    # The CLI reaches the app through an AF_UNIX socket, new in 1803.
    if ([Environment]::OSVersion.Version.Build -lt 17134) {
        Fail 'Draftmux needs Windows 10 version 1803 or later'
    }

    $version = $env:DRAFTMUX_VERSION
    if (-not $version) {
        # /latest redirects to the latest release's tag page.
        try {
            $response = [Net.WebRequest]::Create("$releases/latest").GetResponse()
            $url = $response.ResponseUri.AbsoluteUri
            $response.Close()
        } catch {
            Fail "couldn't reach $releases"
        }
        if ($url -notmatch '/tag/v([^/]+)$') { Fail "no release found at $releases" }
        $version = $Matches[1]
    }
    $version = $version -replace '^v', ''

    $dir = if ($env:DRAFTMUX_INSTALL_DIR) { $env:DRAFTMUX_INSTALL_DIR } else {
        Join-Path $env:LOCALAPPDATA 'Programs\Draftmux'
    }
    $dir = [IO.Path]::GetFullPath($dir)
    $bin = Join-Path $dir 'bin'
    $exe = Join-Path $bin 'draftmux.exe'
    $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Draftmux.lnk'
    $shell = New-Object -ComObject WScript.Shell

    # Only ever replace a previous Draftmux install, checked before anything
    # is written.
    if (Test-Path -LiteralPath $dir) {
        $item = Get-Item -LiteralPath $dir -Force
        $linked = $item.Attributes -band [IO.FileAttributes]::ReparsePoint
        if ($linked -or -not (Test-Path -LiteralPath (Join-Path $dir '.draftmux-bundle') -PathType Leaf)) {
            Fail "refusing to replace $dir, which isn't Draftmux"
        }
    }
    if ((Test-Path -LiteralPath $shortcut) -and $shell.CreateShortcut($shortcut).TargetPath -ne $exe) {
        Fail "refusing to replace $shortcut, which doesn't start this Draftmux"
    }

    $work = Join-Path ([IO.Path]::GetTempPath()) "draftmux-install-$([guid]::NewGuid())"
    New-Item -ItemType Directory -Path $work | Out-Null
    $parent = Split-Path -Parent $dir
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    # Staged next to the target so the swap is a rename.
    $staging = Join-Path $parent ".draftmux-install-$([guid]::NewGuid())"
    try {
        $name = "Draftmux-$version-windows-x86_64"
        $zip = Join-Path $work "$name.zip"
        $sums = Join-Path $work "$name.sha256"
        foreach ($file in @($zip, $sums)) {
            $leaf = Split-Path -Leaf $file
            try {
                Invoke-WebRequest -UseBasicParsing -OutFile $file "$releases/download/v$version/$leaf"
            } catch {
                Fail "couldn't download $leaf"
            }
        }
        $expected = ((Get-Content -LiteralPath $sums -Raw).Trim() -split '\s+')[0]
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash
        if (-not $expected -or $expected -ne $actual) { Fail "checksum mismatch for $name.zip" }

        $extracted = Join-Path $work 'extracted'
        Expand-Archive -LiteralPath $zip -DestinationPath $extracted
        $app = Join-Path $extracted 'draftmux-windows'
        $entries = @(Get-ChildItem -LiteralPath $extracted -Force)
        $complete = ($entries.Count -eq 1) -and
            (Test-Path -LiteralPath (Join-Path $app '.draftmux-bundle') -PathType Leaf) -and
            (Test-Path -LiteralPath (Join-Path $app 'bin\draftmux.exe') -PathType Leaf) -and
            (Test-Path -LiteralPath (Join-Path $app 'share\draftmux\pi-extension\draftmux.ts') -PathType Leaf)
        if (-not $complete) { Fail "$name.zip doesn't hold a complete Draftmux" }

        New-Item -ItemType Directory -Path $staging | Out-Null
        Move-Item -LiteralPath $app -Destination (Join-Path $staging 'app')
        $previous = Join-Path $staging 'previous'
        if (Test-Path -LiteralPath $dir) {
            try {
                Move-Item -LiteralPath $dir -Destination $previous
            } catch {
                Fail "couldn't replace $dir; quit Draftmux and run this again"
            }
        }
        try {
            Move-Item -LiteralPath (Join-Path $staging 'app') -Destination $dir
        } catch {
            if (Test-Path -LiteralPath $previous) { Move-Item -LiteralPath $previous -Destination $dir }
            Fail "couldn't move Draftmux into $dir"
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
    }

    # Minimized, so the console window the app frees at startup doesn't
    # flash up.
    $link = $shell.CreateShortcut($shortcut)
    $link.TargetPath = $exe
    $link.Arguments = 'app'
    $link.WorkingDirectory = $env:USERPROFILE
    $link.WindowStyle = 7
    $link.Description = 'Draftmux'
    $link.Save()

    # Read and written unexpanded, so entries like %USERPROFILE%\bin survive.
    $key = Get-Item -LiteralPath 'HKCU:\Environment'
    $path = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    $paths = @($path -split ';' | Where-Object { $_ })
    if (-not ($paths | Where-Object { $_.TrimEnd('\') -eq $bin })) {
        $path = ($paths + $bin) -join ';'
        Set-ItemProperty -LiteralPath 'HKCU:\Environment' -Name Path -Value $path -Type ExpandString
        # Tells Explorer, and so programs started afterwards, about the new PATH.
        if (-not ('Draftmux.Env' -as [type])) {
            Add-Type -Namespace Draftmux -Name Env -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam,
    string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG
        [void][Draftmux.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero,
            'Environment', 2, 5000, [ref]$result)
        Write-Host "Added $bin to your PATH; open a new terminal to use the draftmux command."
    }
    if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $bin })) {
        $env:Path = "$env:Path;$bin"
    }
    Write-Host "Installed Draftmux $version in $dir"
}

Install-Draftmux
