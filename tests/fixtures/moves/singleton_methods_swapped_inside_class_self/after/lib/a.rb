class Gauge
  class << self
    def high
      limit + 1
    end

    def low
      limit - 1
    end
  end
end
