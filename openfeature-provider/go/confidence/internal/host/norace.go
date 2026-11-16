//go:build !race

package host

func raceAcquire(*byte) {}

func raceRelease(*byte) {}
