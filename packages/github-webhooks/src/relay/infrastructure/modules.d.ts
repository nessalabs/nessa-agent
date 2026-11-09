declare module "*.wasm" {
  const module: WebAssembly.Module
  export default module
}
declare module "*.cedar" {
  const text: string
  export default text
}
