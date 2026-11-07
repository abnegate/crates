require 'json'
require_relative 'ledger'

module Billing
  # An account that can be charged and refunded.
  class Account
    attr_reader :balance, :owner

    MINIMUM_CHARGE = 50

    def initialize(owner, balance = 0)
      @owner = owner
      @balance = balance
      @history = []
    end

    def charge(amount)
      raise ArgumentError, 'amount too small' if amount < MINIMUM_CHARGE

      @balance -= amount
      @history << { type: :charge, amount: amount }
      @balance
    end

    def refund(amount)
      @balance += amount
      @history << { type: :refund, amount: amount }
      @balance
    end

    def to_json(*args)
      { owner: owner, balance: balance }.to_json(*args)
    end

    private

    def audit_trail
      @history.map { |entry| "#{entry[:type]}:#{entry[:amount]}" }
    end
  end

  def self.open(owner)
    Account.new(owner)
  end
end
