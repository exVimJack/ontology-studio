$names = @('reqwest','tokio','uuid','base64','windows-sys','windows','ring','indexmap','hashbrown','tracing','form_urlencoded','url','hyper','once_cell','serde','tokio-util','rustls','sqlx','datafusion','arrow','chrono','futures-util','object_store','parquet')
$lock = Get-Content 'Cargo.lock'
for ($i = 0; $i -lt $lock.Count; $i++) {
    $m = [regex]::Match($lock[$i], '^name = "([^"]+)"$')
    if ($m.Success) {
        $name = $m.Groups[1].Value
        if ($names -contains $name) {
            $verLine = $lock[$i+1]
            $vm = [regex]::Match($verLine, 'version = "([^"]+)"')
            if ($vm.Success) { Write-Output ("{0} @ {1}" -f $name, $vm.Groups[1].Value) }
        }
    }
}
