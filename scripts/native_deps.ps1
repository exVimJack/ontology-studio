$native = @('rusqlite','libsqlite3-sys','sqlite3','pdfium','zstd-sys','brotli-decompressor','ring','schannel','openssl-sys','curl-sys','webview2-com-sys','windows','raw-window-handle','krb5-sys','libssh2-sys')
$lock = Get-Content 'Cargo.lock'
for ($i = 0; $i -lt $lock.Count; $i++) {
    $m = [regex]::Match($lock[$i], '^name = "([^"]+)"$')
    if ($m.Success) {
        $name = $m.Groups[1].Value
        foreach ($n in $native) {
            if ($name -eq $n -or $name -like "$n*") {
                $vm = [regex]::Match($lock[$i+1], 'version = "([^"]+)"')
                Write-Output ("{0} @ {1}" -f $name, $vm.Groups[1].Value)
                break
            }
        }
    }
}
