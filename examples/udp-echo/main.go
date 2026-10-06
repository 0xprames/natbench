// Independent UDP application: no natbench package or worker protocol dependency.
package main

import (
	"flag"
	"fmt"
	"net"
	"os"
	"time"
)

func main() {
	mode := flag.String("mode", "client", "server or client")
	address := flag.String("address", "198.18.0.1:9999", "listen or destination address")
	message := flag.String("message", "hello", "payload to echo")
	flag.Parse()
	if err := run(*mode, *address, *message); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run(mode, address, message string) error {
	if mode != "server" && mode != "client" {
		return fmt.Errorf("mode must be server or client")
	}
	endpoint, err := net.ResolveUDPAddr("udp4", address)
	if err != nil {
		return err
	}
	if mode == "server" {
		socket, err := net.ListenUDP("udp4", endpoint)
		if err != nil {
			return err
		}
		defer socket.Close()
		fmt.Println("READY")
		data := make([]byte, 2048)
		for {
			n, peer, err := socket.ReadFromUDP(data)
			if err != nil {
				return err
			}
			if _, err = socket.WriteToUDP(data[:n], peer); err != nil {
				return err
			}
		}
	}
	socket, err := net.DialUDP("udp4", nil, endpoint)
	if err != nil {
		return err
	}
	defer socket.Close()
	if err = socket.SetDeadline(time.Now().Add(2 * time.Second)); err != nil {
		return err
	}
	if _, err = socket.Write([]byte(message)); err != nil {
		return err
	}
	data := make([]byte, 2048)
	n, err := socket.Read(data)
	if err != nil {
		return err
	}
	if string(data[:n]) != message {
		return fmt.Errorf("unexpected payload %q", data[:n])
	}
	fmt.Println("PASS", message)
	return nil
}
