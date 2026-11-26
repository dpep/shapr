class Gauge
  def self.singleton_method_added(name)
    (@seen ||= []) << name
  end

  class << self
    def low
      limit - 1
    end

    def high
      limit + 1
    end
  end
end
