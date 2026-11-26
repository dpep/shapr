class Gauge
  class << self
    def low
      limit - 1
    end

    def high
      limit + 1
    end
  end
end
