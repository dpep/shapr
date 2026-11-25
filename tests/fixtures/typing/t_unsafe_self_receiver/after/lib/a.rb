# typed: true
class A
  extend T::Sig

  def run
    T.unsafe(self).secret
  end

  private

  def secret
    42
  end
end
