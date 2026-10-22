# NUS Client

Cliente de escritorio BLE para dispositivos con **Nordic UART Service** (NUS),
al estilo de *Serial Bluetooth Terminal*. Escrito en Rust con
[btleplug](https://github.com/deviceplug/btleplug) + [egui](https://github.com/emilk/egui).
Funciona en Linux (BlueZ) y Windows 10/11 (WinRT).

## Funciones

- Escaneo de dispositivos (nombre, dirección, RSSI, marca `[NUS]`), filtro "Sólo NUS".
  Doble clic en un dispositivo para conectar.
- Usa por defecto NUS: servicio `6E400001-…`, escritura `6E400002-…`, notificación `6E400003-…`.
- Panel **Servicios**: muestra todos los servicios/características descubiertos con sus
  propiedades y permite elegir otra característica de escritura y/o de notificación.
  "Guardar como predeterminadas" hace que se elijan automáticamente al conectar.
- Terminal con RX/TX en colores, hora opcional, eco local, copiar y limpiar.
- Entrada de texto con fin de línea configurable (Ninguno / CR / LF / CR+LF) e historial (↑/↓).
- **Macros**: botones de comandos predefinidos. Modo *Enviar* (envía al pulsar) o
  *Insertar* (lo copia a la entrada para completarlo). Editables desde la app.
- **Log** de sesión a archivo con timestamps (ruta automática o elegida con 📁).
- Escrituras troceadas según MTU (o tamaño fijo), con o sin respuesta.

## Configuración

Se guarda automáticamente en:

- Linux: `~/.config/nus-client/config.toml`
- Windows: `%APPDATA%\nus-client\config\config.toml`

Ejemplo (se puede editar a mano con la app cerrada):

```toml
line_ending = "CRLF"          # NONE | CR | LF | CRLF
chunk_size = 0                # 0 = automático (MTU - 3)
write_with_response = false
timestamps = true
echo = true
only_nus = false
default_write_uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
default_notify_uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"

[[macros]]
name = "Estado"
text = "AT+STATUS"
mode = "send"                 # send | insert
```

Los logs automáticos van a `~/.local/share/nus-client/logs/` (Linux) o
`%APPDATA%\nus-client\data\logs\` (Windows).

## Compilar

### Linux

```sh
sudo apt install libdbus-1-dev pkg-config   # Debian/Ubuntu
cargo build --release
./target/release/nus-client
```

Requiere el servicio `bluetooth` (BlueZ) activo. El usuario debe poder usar D-Bus
de BlueZ (normalmente ya es así en escritorio).

### Windows

```powershell
cargo build --release
.\target\release\nus-client.exe
```

Sin dependencias extra (toolchain MSVC recomendada). Bluetooth debe estar activado
en la configuración del sistema.

En Windows la GUI usa DX12 (con OpenGL de respaldo) en lugar de Vulkan, porque
algunos drivers/capas Vulkan cierran la app al crear la ventana. Se puede forzar
otro backend con la variable `WGPU_BACKEND` (`vulkan`, `dx12`, `gl`).

## Notas

- Si un dispositivo no aparece en Linux tras conectarlo antes, puede estar en caché de
  BlueZ: aparecerá al escanear de todos modos ("Vaciar" limpia la lista visible).
- Si los mensajes largos se cortan, baja el *tamaño de trozo* en ⚙ Ajustes (p.ej. 20).

## Tests

```sh
cargo test
```
