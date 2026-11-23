package com.spotify.confidence.sdk.internal.host;

import static org.assertj.core.api.Assertions.assertThat;
import static org.assertj.core.api.Assertions.assertThatThrownBy;

import java.util.List;
import org.junit.jupiter.api.Test;

/** Ports the Go host's module_test.go: fixture compilation and invalid-module rejection. */
class ModuleTest {

  @Test
  void compilesTheFixture() {
    Module module = Module.parse(HostTestSupport.fixture());
    assertThat(module.minPages).isPositive();
    assertThat(module.maxPages).isGreaterThanOrEqualTo(module.minPages);
    for (String name :
        List.of(
            "allocated_bytes",
            "metrics_snapshot",
            "observe_client_resolve_latency",
            "unpack_bitsets")) {
      assertThat(module.optional).contains(name);
    }
  }

  @Test
  void rejectsModules() {
    record Case(String name, byte[] wasm, String detail) {}
    List<Case> cases =
        List.of(
            new Case("garbage", "not wasm".getBytes(), "parse"),
            new Case("no memory import", emptyModule(), "expected one imported memory"),
            new Case("wrong memory name", memoryImportModule("env", "memory", true), "host.memory"),
            new Case(
                "missing exports", memoryImportModule("host", "memory", true), "missing export"));
    for (Case c : cases) {
      assertThatThrownBy(() -> Module.parse(c.wasm()))
          .as(c.name())
          .isInstanceOf(InvalidModuleException.class)
          .hasMessageContaining(c.detail());
    }
  }

  @Test
  void rejectsAnUnsharedMemoryImport() {
    assertThatThrownBy(() -> Module.parse(memoryImportModule("host", "memory", false)))
        .isInstanceOf(InvalidModuleException.class)
        .hasMessageContaining("shared");
  }

  // The wasm builders the Go module tests use: a module that imports one memory and nothing
  // else (min 1, max 2), shared or not.

  private static byte[] emptyModule() {
    return new byte[] {0, 'a', 's', 'm', 1, 0, 0, 0};
  }

  private static byte[] memoryImportModule(String module, String name, boolean shared) {
    int flags = shared ? 0x03 : 0x01;
    byte[] mod = module.getBytes();
    byte[] field = name.getBytes();
    byte[] imp = new byte[1 + 1 + mod.length + 1 + field.length + 4];
    int i = 0;
    imp[i++] = 1; // one import
    imp[i++] = (byte) mod.length;
    System.arraycopy(mod, 0, imp, i, mod.length);
    i += mod.length;
    imp[i++] = (byte) field.length;
    System.arraycopy(field, 0, imp, i, field.length);
    i += field.length;
    imp[i++] = 0x02; // memory
    imp[i++] = (byte) flags; // has max, and shared when set
    imp[i++] = 1; // min pages
    imp[i++] = 2; // max pages
    byte[] b = emptyModule();
    byte[] out = new byte[b.length + 2 + imp.length];
    System.arraycopy(b, 0, out, 0, b.length);
    out[b.length] = 2; // import section id
    out[b.length + 1] = (byte) imp.length; // section length (fits a byte here)
    System.arraycopy(imp, 0, out, b.length + 2, imp.length);
    return out;
  }
}
