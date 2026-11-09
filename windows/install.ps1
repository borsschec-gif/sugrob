# Устанавливает Сугроб для текущего пользователя, права администратора не нужны. Запуск:
# irm https://raw.githubusercontent.com/borsschec-gif/sugrob/windows/windows/install.ps1 | iex
# Всё внутри блока: exit и настройки не должны задеть окно PowerShell, из которого запустили.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $repo = 'borsschec-gif/sugrob'
    $dir = Join-Path $env:LOCALAPPDATA 'Programs\Sugrob'
    $exe = Join-Path $dir 'Sugrob.exe'
    $new = "$exe.new"

    # В releases/latest лежит версия для Mac, поэтому ищем самый новый тег win-v по номеру.
    Write-Host 'Ищу последнюю версию…'
    $releases = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'Sugrob-installer' } -Uri "https://api.github.com/repos/$repo/releases?per_page=100"
    $best = $null
    $bestVersion = $null
    foreach ($release in $releases) {
        if ($release.draft -or $release.prerelease) { continue }
        if ($release.tag_name -notmatch '^win-v(\d+(\.\d+){1,3})$') { continue }
        if (-not ($release.assets | Where-Object { $_.name -eq 'Sugrob.exe' })) { continue }
        $version = [version]$Matches[1]
        if ($null -eq $bestVersion -or $version -gt $bestVersion) {
            $best = $release
            $bestVersion = $version
        }
    }
    if ($null -eq $best) { throw 'Сборки для Windows пока не выпущены.' }

    Write-Host "Скачиваю Сугроб $bestVersion…"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/$repo/releases/download/$($best.tag_name)/Sugrob.exe" -OutFile $new

        # Останавливаем только копию из этой папки.
        $running = @(Get-Process -Name Sugrob -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe })
        if ($running.Count -gt 0) {
            $running | Stop-Process -Force
            $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
        }

        # Файл может быть занят ещё мгновение после выхода программы.
        for ($try = 1; ; $try++) {
            try {
                Move-Item -Force -LiteralPath $new -Destination $exe
                break
            } catch {
                if ($try -ge 10) { throw }
                Start-Sleep -Milliseconds 300
            }
        }
    } finally {
        Remove-Item -Force -LiteralPath $new -ErrorAction SilentlyContinue
    }
    Remove-Item -Force -LiteralPath "$exe.old" -ErrorAction SilentlyContinue

    Start-Process -FilePath $exe
    Write-Host 'Готово. Сугроб живёт в трее, автозапуск включается там же: «Запускать при входе».'
}
