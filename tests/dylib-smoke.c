// Minimal runtime check for a packaged Apple libcronet.dylib.
//
// The CI job links this against lib/<target>/libcronet.dylib, which proves the
// dylib's install name, exported public C API, ad-hoc signature, and transitive
// system frameworks all work at runtime. It is only executed on macOS arm64;
// iOS slices cannot run outside a simulator or device.

#include <stdbool.h>
#include <stdio.h>
#include <string.h>

#include "cronet.idl_c.h"

int main(void) {
  Cronet_EnginePtr engine = Cronet_Engine_Create();
  if (engine == NULL) {
    fprintf(stderr, "Cronet_Engine_Create returned NULL\n");
    return 1;
  }

  Cronet_String version = Cronet_Engine_GetVersionString(engine);
  if (version == NULL || strncmp(version, "154.", 4) != 0) {
    fprintf(stderr, "unexpected Cronet version: %s\n",
            version == NULL ? "<null>" : version);
    Cronet_Engine_Destroy(engine);
    return 1;
  }

  printf("libcronet.dylib loaded, version %s\n", version);
  Cronet_Engine_Destroy(engine);
  return 0;
}
