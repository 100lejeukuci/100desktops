# remote-desk

Prototip aplikacije za daljinski pristup ekranu (zamena za AnyDesk) za **Windows 11**, napisan u Rustu.
Cilj je što manja latencija: hvatanje ekrana, kodiranje i dekodiranje rade na GPU-u, a prenos ide
preko sopstvenog UDP protokola.

## Šta radi

- Hvatanje ekrana (DXGI Desktop Duplication), 60 fps
- Hardverski H.264 enkoder (Media Foundation: AMD, NVIDIA, Intel) i GPU dekoder
- Sopstveni UDP protokol: deljenje kadra na pakete, oporavak od gubitka preko NACK-a (ponovno slanje) i zahteva za keyframe
- Daljinska kontrola miša i tastature (`SendInput`), sa pouzdanim slanjem tastera i dugmadi
- Host odobrava svaku sesiju (pregled i kontrola, samo pregled, ili odbijanje); `Ctrl+Alt+Shift+F12` na hostu je hitni prekid

Izmereno na jednom računaru (loopback, AMD RX 6900 XT, 1920x1080): 60 fps, oko 6,5 ms od hvatanja do prikaza,
60 fps i pri 1% gubitka paketa. **Nije još testirano na dva računara.**

## Pokretanje

Gotov program je `dist/remote-desk.exe` (bez instalacije). Za test na dva računara pogledaj [TEST-2PC.md](TEST-2PC.md).

```
remote-desk host [--port 7000]              # deli ovaj ekran, pita da li da dozvoli vezu
remote-desk view <ip-hosta>:7000            # gleda i kontroliše host
remote-desk local                           # benchmark na jednom računaru (bez mreže)
remote-desk probe                           # test prozor koji ispisuje primljeni unos
```

Host mora imati GPU sa hardverskim H.264 enkoderom, a firewall mora dozvoliti dolazni UDP na portu 7000.

## Build

Potrebni su [Rust](https://rustup.rs) (MSVC toolchain) i Visual Studio Build Tools sa C++ workload-om.

```
cargo build --release
```

Izlaz je `target/release/remote-desk.exe`. C runtime se linkuje statički (`.cargo/config.toml`), pa exe radi
i bez Visual C++ redistributable paketa.

## Struktura

| Fajl | Uloga |
|---|---|
| `src/capture.rs` | hvatanje ekrana (DXGI) |
| `src/encoder.rs` | BGRA→NV12 na GPU-u i hardverski H.264 enkoder |
| `src/decoder.rs` | H.264 dekoder |
| `src/viewer.rs` | prozor, prikaz i hvatanje miša/tastature |
| `src/proto.rs` | UDP protokol, paketizacija, reassembler, NACK |
| `src/input.rs` | pouzdano slanje unosa (viewer) i `SendInput` (host) |
| `src/host.rs`, `src/client.rs` | sesija na hostu i na viewer-u |

## Ograničenja

- **Nema enkripcije ni provere identiteta.** Koristi samo na mreži kojoj veruješ.
- Samo prvi monitor; nema prikaza kursora sa hosta; nema zvuka, clipboard-a ni prenosa fajlova.
- Win, Alt+Tab i Ctrl+Alt+Del ostaju lokalni na viewer-u. UAC prozori i login ekran se ne mogu kontrolisati
  (za to treba Windows servis).
- Radi samo u istoj mreži; za internet treba signaling server i NAT traversal (STUN/TURN).
