/** Safe to display: protocol ranges contain no credential or identity information. */
export class NessaProtocolCompatibilityError extends Error {
  readonly code = "protocol_incompatible"

  constructor(
    readonly clientMinVersion: number,
    readonly clientMaxVersion: number,
    readonly serverMinVersion: number,
    readonly serverMaxVersion: number,
  ) {
    super(
      `Cannot connect: no compatible product protocol version. ` +
        `Client supports ${clientMinVersion}–${clientMaxVersion}; ` +
        `gateway supports ${serverMinVersion}–${serverMaxVersion}. ` +
        `Use client and gateway versions with an overlapping protocol range. ` +
        `No credential was sent.`,
    )
    this.name = "NessaProtocolCompatibilityError"
  }
}
