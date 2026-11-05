// En Windows, incrusta el ícono en el .exe para que lo muestren el Explorador, los accesos
// directos y la barra de tareas. Se regenera con scripts/build-icons.sh.
fn main() {
    println!("cargo:rerun-if-changed=assets/nus-client.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/nus-client.ico");
        resource
            .compile()
            .expect("no se pudo incrustar el ícono en el ejecutable");
    }
}
