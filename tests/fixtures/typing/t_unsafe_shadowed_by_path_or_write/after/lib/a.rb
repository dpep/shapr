module Freezer
  def self.unsafe(x) = x.dup.freeze
end

class A
end
A::T ||= Freezer

class A
  def run(x)
    T.unsafe(x) << 1
  end
end
