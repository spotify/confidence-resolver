package com.spotify.confidence.sdk;

public class TestAccountStateProvider implements AccountStateProvider {
  private byte[] stateBytes;
  private String account;
  private final byte[] initialStateBytes;
  private final String initialAccount;

  public TestAccountStateProvider(byte[] stateBytes, String account) {
    this.initialStateBytes = stateBytes;
    this.initialAccount = account;
  }

  @Override
  public void reload() {
    this.stateBytes = new byte[initialStateBytes.length];
    System.arraycopy(initialStateBytes, 0, stateBytes, 0, initialStateBytes.length);
    this.account = initialAccount;
  }

  @Override
  public byte[] provide() {
    return stateBytes;
  }

  @Override
  public String accountId() {
    return account;
  }
}
