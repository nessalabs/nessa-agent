@id("directory.read")
permit(principal, action, resource)
when {
    context.live && context.within && context.read && context.operation == "read"
};
@id("directory.write")
permit(principal, action, resource)
when {
    context.live && context.within && context.write && context.operation == "write"
};
