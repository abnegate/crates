package clock

import "time"

// Clock returns the current time.
type Clock interface {
	Now() time.Time
}

type systemClock struct{}

func (systemClock) Now() time.Time {
	return time.Now().UTC()
}

// Since reports how long ago start was according to clock.
func Since(clock Clock, start time.Time) time.Duration {
	return clock.Now().Sub(start)
}